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
// which silently produces no encoded media (blank tile). We scale the source DOWN
// PRESERVING ASPECT (no more squishing an ultrawide into 16:9) — see fit_even.

// ---- Screen-share quality: a resolution preset (box), fps, and bitrate the user picks.
// The preset's short side is the line count the source is scaled to, preserving aspect,
// so the published frame matches the monitor's real shape (an ultrawide stays ultrawide
// AND keeps the preset's lines).
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
        // "Game High": 1080p, 30fps, 12 Mbps over hardware NVENC. Aspect-preserving +
        // ÷16-aligned (a 21:9 ultrawide publishes 2560x1072). Safe default; higher/lower
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

/// The longest side a share is published at: H.264 (incl. NVENC) stops at 4096 per dimension, and a share must
/// stay decodable when it falls back to H.264.
pub const SHARE_MAX_DIM: u32 = 4096;

/// The share's published size for a (sw,sh) source under a preset box (max_w,max_h) — the rule Tin Hat Web shares
/// (`shareFit.ts`, vectors in testvectors/share-fit/vectors.json).
///
/// The preset's SHORT side is its line count: "1080p" (1920x1080) means 1080 lines, so the source's short side is
/// scaled to it and the long side follows the source's aspect: a 21:9 5120x2160 publishes 2560x1072 at 1080p, a 16:9
/// screen 1920x1072 as before, a portrait 1080x1920 screen 1072x1920. (The old rule fitted the source into the 16:9
/// box itself, so the width bound first on any wider screen: 1080p on 21:9 came out 1920x800, 1440p 2560x1072.)
/// Never upscaled; the long side at most [`SHARE_MAX_DIM`]. Each side rounded to the nearest pixel, then aligned down to a multiple of 16:
/// H.264 (incl. NVENC / libwebrtc) needs ÷16 dimensions — a non-aligned width (e.g. 3412) silently encodes to a
/// BLACK frame for peers even though local capture looks fine. The ≤15px aspect drift is imperceptible.
/// An unknown source (0x0) gets the box itself.
pub fn fit_even(sw: u32, sh: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if sw == 0 || sh == 0 {
        return (align16(max_w).max(16), align16(max_h).max(16));
    }
    let lines = max_w.min(max_h) as f64;
    let scale = (lines / sw.min(sh) as f64)
        .min(SHARE_MAX_DIM as f64 / sw.max(sh) as f64)
        .min(1.0);
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
    KeyProvider::new(matrix_key_provider_options())
}

/// The options behind [`matrix_key_provider`] (split out so the unit tests can pin them).
pub(crate) fn matrix_key_provider_options() -> KeyProviderOptions {
    let mut opts = KeyProviderOptions::default(); // salt already "LKFrameEncryptionKey"
    opts.key_derivation_algorithm = KeyDerivationAlgorithm::HKDF; // EC uses HKDF (livekit default is PBKDF2)
    // element-call's index space is % 256, but libwebrtc's ParticipantKeyHandler clamps
    // the ring to MAX_KEYRING_SIZE=255 slots while decryptFrame bounds-checks against the
    // UNCLAMPED option — key_ring_size=256 let a frame with trailer index 255 read one
    // past the ring (OOB/abort). 255 turns that frame into a clean kMissingKey drop
    // instead; install_peer_key_at refuses index 255 with a log for the same reason.
    opts.key_ring_size = 255;
    // A frame that meets a cryptor which isn't enabled (just attached, or already disabled at
    // teardown while its receiver lives on) is DROPPED, never passed through: upstream's
    // default forwards it untransformed, i.e. ciphertext straight into the decoder (black
    // frames; a hardware decoder fed garbage) and, on the send side, an unencrypted frame to
    // the SFU. Missing keys were already a drop; this closes the disabled-cryptor path too.
    // The web's livekit-client worker drops in the same window (state unknown → drop).
    opts.discard_frame_when_cryptor_not_ready = true;
    opts
}

#[cfg(test)]
mod key_provider_tests {
    use super::*;

    #[test]
    fn frames_meeting_a_cryptor_that_is_not_ready_are_dropped() {
        let o = matrix_key_provider_options();
        assert!(o.discard_frame_when_cryptor_not_ready, "never pass a frame through a disabled cryptor");
        assert!(!KeyProviderOptions::default().discard_frame_when_cryptor_not_ready, "vendored default stays upstream's");
    }

    /// The EFFECTIVE native values (libwebrtc's own options, read back): the copy constructor in
    /// libwebrtc drops the discard flag, so the Rust option alone proved nothing (it was
    /// uninitialized native memory on every build up to 12.1.2); and no magic "unencrypted"
    /// trailer is installed, so nothing bypasses decryption.
    #[test]
    fn the_native_key_provider_really_discards_and_has_no_bypass_trailer() {
        let (discard, magic_len) = matrix_key_provider().effective_options();
        assert!(discard, "libwebrtc must see discard_frame_when_cryptor_not_ready = true");
        assert_eq!(magic_len, 0, "no SIF / magic trailer may be installed");
    }

    /// 12.1.5 early receiver attach: receiver cryptors are keyed by the NATIVE receiver object
    /// (the receiver track id is the remote SDP's choice and may repeat). The key must be the
    /// same whichever wrapper asks (on_track's transceiver vs. the track's at TrackSubscribed)
    /// and differ between receivers.
    #[test]
    fn a_receiver_has_one_native_key_whichever_wrapper_asks() {
        use livekit::webrtc::{
            peer_connection_factory::{PeerConnectionFactory, RtcConfiguration},
            rtp_transceiver::{RtpTransceiverDirection, RtpTransceiverInit},
            MediaType,
        };
        let factory = PeerConnectionFactory::default();
        let pc = factory.create_peer_connection(RtcConfiguration::default()).expect("pc");
        let init = || RtpTransceiverInit {
            direction: RtpTransceiverDirection::RecvOnly,
            stream_ids: vec![],
            send_encodings: vec![],
        };
        let a = pc.add_transceiver_for_media(MediaType::Video, init()).expect("video transceiver");
        let b = pc.add_transceiver_for_media(MediaType::Audio, init()).expect("audio transceiver");
        let ka = a.receiver().native_key();
        assert_ne!(ka, 0);
        assert_eq!(ka, a.receiver().native_key(), "same receiver, new wrapper, same key");
        let listed: Vec<usize> = pc.transceivers().iter().map(|t| t.receiver().native_key()).collect();
        assert!(listed.contains(&ka), "the PeerConnection's own transceiver list gives the same key");
        assert_ne!(ka, b.receiver().native_key(), "two receivers, two keys");
        pc.close();
    }

    /// 12.1.3 C2: an add_track answer whose sid is already one of our live tracks (or empty) is
    /// refused before the publication/cryptor maps are touched (a replaced sender cryptor would
    /// detach encryption from a track that keeps sending).
    #[test]
    fn a_colliding_add_track_answer_is_refused() {
        use livekit::participant::check_add_track_answer;
        let live = || vec!["TR_mic".to_string(), "TR_cam".to_string()];
        assert!(check_add_track_answer(live(), "TR_share").is_ok());
        assert!(check_add_track_answer(live(), "TR_mic").is_err(), "a sid already in use");
        assert!(check_add_track_answer(live(), "TR_cam").is_err(), "a sid already in use");
        assert!(check_add_track_answer(live(), "").is_err(), "an empty sid");
        assert!(check_add_track_answer(Vec::<String>::new(), "TR_first").is_ok());
    }

    /// 12.1.3 C2b: the sid check is atomic with the inserts — concurrent publishes answered with the
    /// same sid (join: mic + camera + share; reconnect: the republish) get exactly ONE reservation;
    /// a held or live sid is refused; a released reservation frees the sid; an unpublish in
    /// progress (tombstone) refuses a publish answered with its sid.
    #[test]
    fn concurrent_publishes_answered_with_one_sid_get_one_reservation() {
        use livekit::participant::SidReservations;
        use std::sync::{Arc, Barrier, Mutex};
        let res = Arc::new(SidReservations::default());
        let live: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec!["TR_mic".into()]));
        for round in 0..50 {
            let sid = format!("TR_dup{round}");
            let gate = Arc::new(Barrier::new(8));
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    let (res, live, gate, sid) = (res.clone(), live.clone(), gate.clone(), sid.clone());
                    std::thread::spawn(move || {
                        gate.wait();
                        res.reserve(&sid, || live.lock().unwrap().clone()).ok().map(|g| {
                            // the publication "is inserted" while the reservation is held
                            live.lock().unwrap().push(sid.clone());
                            drop(g);
                        })
                    })
                })
                .collect(); // all 8 spawned before any join (the barrier needs them all)
            let wins: Vec<_> = handles.into_iter().map(|t| t.join().unwrap()).filter(|w| w.is_some()).collect();
            assert_eq!(wins.len(), 1, "round {round}: exactly one publish may take the sid");
        }
        // live and held sids are refused; a dropped reservation frees its sid
        assert!(res.reserve("TR_mic", || live.lock().unwrap().clone()).is_err());
        let held = res.reserve("TR_new", || Vec::<String>::new()).unwrap();
        assert!(res.reserve("TR_new", || Vec::<String>::new()).is_err(), "held by a publish in flight");
        drop(held);
        assert!(res.reserve("TR_new", || Vec::<String>::new()).is_ok(), "released");
        // an unpublish in progress holds its sid
        let tomb = res.tombstone("TR_gone");
        assert!(res.reserve("TR_gone", || Vec::<String>::new()).is_err(), "unpublish in progress");
        drop(tomb);
        assert!(res.reserve("TR_gone", || Vec::<String>::new()).is_ok());
        assert!(res.reserve("", || Vec::<String>::new()).is_err(), "empty sid");
        // Astra's schedule: publish A still holds S when unpublish A takes its tombstone; A's
        // reservation goes first — S must stay held until the unpublish is done too (counted).
        let a = res.reserve("TR_S", || Vec::<String>::new()).unwrap();
        let unpub = res.tombstone("TR_S");
        drop(a);
        assert!(res.reserve("TR_S", || Vec::<String>::new()).is_err(), "still being unpublished");
        let unpub2 = res.tombstone("TR_S"); // an overlapping second unpublish
        drop(unpub);
        assert!(res.reserve("TR_S", || Vec::<String>::new()).is_err(), "the second unpublish still holds it");
        drop(unpub2);
        assert!(res.reserve("TR_S", || Vec::<String>::new()).is_ok(), "free once every holder let go");
    }

    /// 12.1.3 C2b: every publish / unpublish of ONE local track runs under that track's lock —
    /// operations on the same track never interleave (assembly-039's / Astra's same-track teardown
    /// schedules), different tracks run concurrently, and an unused entry is forgotten.
    #[test]
    fn same_track_operations_are_serialized_other_tracks_are_not() {
        use livekit::participant::TrackOpLocks;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(4).build().unwrap();
        rt.block_on(async {
            let locks = Arc::new(TrackOpLocks::default());
            let inside = Arc::new(AtomicUsize::new(0));
            let max_same = Arc::new(AtomicUsize::new(0));
            let mut tasks = Vec::new();
            for _ in 0..16 {
                let (locks, inside, max_same) = (locks.clone(), inside.clone(), max_same.clone());
                tasks.push(tokio::spawn(async move {
                    let _g = locks.lock("cid-T").await; // a publish or an unpublish of track T
                    let now = inside.fetch_add(1, Ordering::SeqCst) + 1;
                    max_same.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(5)); // the operation's work (awaits in the real one)
                    tokio::task::yield_now().await;
                    inside.fetch_sub(1, Ordering::SeqCst);
                }));
            }
            for t in tasks {
                t.await.unwrap();
            }
            assert_eq!(max_same.load(Ordering::SeqCst), 1, "two operations of the same track interleaved");
            // other tracks are not held up by T
            let _t = locks.lock("cid-T").await;
            let _u = locks.lock("cid-U").await; // would hang here if T's lock blocked other tracks
        });
    }

    #[test]
    fn matrix_key_provider_keeps_the_element_call_contract() {
        let o = matrix_key_provider_options();
        assert_eq!(o.ratchet_salt, b"LKFrameEncryptionKey".to_vec());
        assert_eq!(o.key_ring_size, 255);
        assert!(matches!(o.key_derivation_algorithm, KeyDerivationAlgorithm::HKDF));
        // and the provider builds with them (the options cross the FFI into libwebrtc)
        let _ = matrix_key_provider();
    }
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
        let missing: Vec<&str> = ["cudaupload", "cudaconvertscale", "cudadownload"]
            .into_iter()
            .filter(|n| gst::ElementFactory::find(n).is_none())
            .collect();
        if !missing.is_empty() {
            // Said once: without it every share silently took the CPU path (a 12.0.0 Flatpak log had nothing).
            let nvcodec = gst::Registry::get().find_plugin("nvcodec").is_some();
            eprintln!(
                "[capture] GPU (CUDA) scale unavailable: no {} (GStreamer nvcodec plugin {}) — shares scale on the CPU",
                missing.join("/"),
                if nvcodec { "loaded, CUDA elements not registered" } else { "not found" }
            );
        }
        missing.is_empty()
    })
}

/// Which capture path a share tries, as (GPU with a known size, GPU learning the size from the first frame); neither =
/// the CPU convert + lanczos path. `cuda` = the CUDA filters are registered (an NVIDIA driver AND NVRTC; a bundled
/// libnvrtc alone registers nothing on a machine without NVIDIA, so such a machine stays on the CPU path), `d3d11` =
/// the Windows D3D11 scaler. No size known (KDE's portal gives none for a window and often none for a monitor) → the
/// CUDA path that pins its output from the FIRST frame; Linux only, on Windows the picker always knows the size.
pub(crate) fn share_capture_plan(
    size_known: bool,
    gpu_pref_ok: bool,
    cuda: bool,
    d3d11: bool,
    force_cpu: bool,
    windows: bool,
) -> (bool, bool) {
    let has_gpu_elements = cuda || (windows && d3d11);
    let gpu_ok = gpu_pref_ok && has_gpu_elements && !force_cpu;
    let want_gpu = size_known && gpu_ok;
    let want_gpu_learn = !want_gpu && gpu_ok && !windows && cuda;
    (want_gpu, want_gpu_learn)
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
    let desc = cuda_presized_desc(&src, out_w, out_h, fps);
    gst::parse::launch(&desc)?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("cuda capture: parsed element is not a Pipeline"))
}

/// The launch line of [`build_capture_pipeline_cuda`] (the pre-sized GPU capture: its output caps are final at build
/// time, so nothing renegotiates mid-stream) for a source head; the share-start gate feeds it a videotestsrc.
pub(crate) fn cuda_presized_desc(src: &str, out_w: u32, out_h: u32, fps: u32) -> String {
    format!(
        "{src} ! \
         videorate drop-only=true ! video/x-raw(ANY),framerate={fps}/1 ! \
         cudaupload ! cudaconvertscale name=vscale ! \
         video/x-raw(memory:CUDAMemory),format=I420,width={out_w},height={out_h},pixel-aspect-ratio=1/1 ! \
         cudadownload ! \
         queue name=outq max-size-buffers=4 leaky=downstream ! \
         appsink name=sink emit-signals=false max-buffers=2 drop=true sync=false"
    )
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
         queue name=outq max-size-buffers=4 leaky=downstream ! \
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
                    // What the portal actually hands the scaler: memory:DMABuf on the CPU path is a GPU->CPU read of the
                    // whole source per frame (the expensive case), system memory is the compositor's own copy.
                    let mem = caps.features(0).map(|f| f.to_string()).unwrap_or_default();
                    let fmt = s.get::<&str>("format").unwrap_or("?");
                    eprintln!(
                        "[lk] capture real {rw}x{rh} -> publishing {ow}x{oh} (aspect-correct, {path}; input {fmt} {})",
                        if mem.is_empty() { "memory:SystemMemory" } else { mem.as_str() }
                    );
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
         d3d11convert name=vscale add-borders=false ! \
         {tail} ! \
         queue name=outq max-size-buffers=4 leaky=downstream ! \
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
/// 12.1.2: at most 8. The field ceiling is a share while a GAME loads every core: builder with all 24 threads busy,
/// 5120x2160 -> 3408x1440 ran 44 fps on 4 threads and 55 on 8 (-> 2560x1072: 61 -> 72), and a tester's 5K shares sat at
/// 32-37 fps on the 4-thread path (STREAM-QUALITY-FINDINGS.md). The threads only work while a frame is scaling.
pub(crate) fn cpu_capture_threads() -> u32 {
    cpu_capture_threads_for(std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(2))
}

pub(crate) fn cpu_capture_threads_for(cores: u32) -> u32 {
    (cores / 2).clamp(1, 8)
}

/// 12.1.2: where a share's frames go missing, said every 5 s while it runs. A 60 fps cap that publishes 35 fps used to
/// leave nothing to tell the compositor's delivery from the scaler from the encoder (the field logs only had "fed N
/// frames"). Counts buffers at the capture source's pad, at the scaler's input (after the rate cap) and at the appsink;
/// times the scaler (in -> out) and the whole synchronous capture chain (source pad -> the output queue: rate cap,
/// upload, scale, download, which all run on the source's streaming thread, so a slow chain holds the source back);
/// then names the stage that set the rate. RC2: the chain timing exists because RC1's first-start path read
/// "source 19 fps" and blamed the compositor while its own CUDA chain was what held pipewiresrc back
/// (the 12.1.2 RC1 field diagnosis, F6). Probes only count; the reporter holds a weak ref and ends with the pipeline.
pub(crate) fn install_cadence_probes(pipeline: &gst::Pipeline, cap_fps: u32, path: &'static str) {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    use std::sync::{Arc, Mutex};
    #[derive(Default)]
    struct Counts {
        src: AtomicU64,
        scale_in: AtomicU64,
        out: AtomicU64,
        scale_sum_us: AtomicU64,
        scale_max_us: AtomicU64,
        scale_n: AtomicU64,
        chain_sum_us: AtomicU64,
        chain_max_us: AtomicU64,
        chain_n: AtomicU64,
    }
    let c = Arc::new(Counts::default());
    // a stopwatch started at one pad and read at a later pad on the SAME streaming thread
    let timed = |c: &Arc<Counts>, start: &gst::Pad, end: &gst::Pad, which: fn(&Counts) -> (&AtomicU64, &AtomicU64, &AtomicU64)| {
        let t0: Arc<Mutex<Option<std::time::Instant>>> = Arc::new(Mutex::new(None));
        {
            let t0 = t0.clone();
            start.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                if let Ok(mut t) = t0.lock() {
                    *t = Some(std::time::Instant::now());
                }
                gst::PadProbeReturn::Ok
            });
        }
        let c = c.clone();
        end.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
            if let Some(t) = t0.lock().ok().and_then(|mut t| t.take()) {
                let us = t.elapsed().as_micros() as u64;
                let (sum, max, n) = which(&c);
                sum.fetch_add(us, Relaxed);
                max.fetch_max(us, Relaxed);
                n.fetch_add(1, Relaxed);
            }
            gst::PadProbeReturn::Ok
        });
    };
    let src_pad = pipeline.iterate_sources().into_iter().flatten().next().and_then(|e| e.static_pad("src"));
    if let Some(pad) = &src_pad {
        let c = c.clone();
        pad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
            c.src.fetch_add(1, Relaxed);
            gst::PadProbeReturn::Ok
        });
    }
    let vscale = pipeline.by_name("vscale");
    let (scale_in_pad, scale_out_pad) = (
        vscale.as_ref().and_then(|v| v.static_pad("sink")),
        vscale.as_ref().and_then(|v| v.static_pad("src")),
    );
    if let Some(pad) = &scale_in_pad {
        let c = c.clone();
        pad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
            c.scale_in.fetch_add(1, Relaxed);
            gst::PadProbeReturn::Ok
        });
    }
    if let (Some(i), Some(o)) = (&scale_in_pad, &scale_out_pad) {
        timed(&c, i, o, |c| (&c.scale_sum_us, &c.scale_max_us, &c.scale_n));
    }
    let outq_in = pipeline.by_name("outq").and_then(|q| q.static_pad("sink"));
    if let (Some(s), Some(q)) = (&src_pad, &outq_in) {
        timed(&c, s, q, |c| (&c.chain_sum_us, &c.chain_max_us, &c.chain_n));
    }
    if let Some(pad) = pipeline.by_name("sink").and_then(|s| s.static_pad("sink")) {
        let c = c.clone();
        pad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
            c.out.fetch_add(1, Relaxed);
            gst::PadProbeReturn::Ok
        });
    }
    let (has_scaler, has_chain) = (scale_in_pad.is_some() && scale_out_pad.is_some(), src_pad.is_some() && outq_in.is_some());
    if !has_scaler || !has_chain {
        eprintln!("[lk/cadence] {path}: not measured here: {}{}", if has_scaler { "" } else { "scaler " }, if has_chain { "" } else { "capture chain" });
    }
    let weak = pipeline.downgrade();
    let src_pad = src_pad.map(|p| p.downgrade());
    let _ = std::thread::Builder::new().name("share-cadence".into()).spawn(move || {
        let period = std::time::Duration::from_secs(5);
        let mut caps_seen = false;
        let mut window = std::time::Instant::now();
        let all = |c: &Counts| {
            [&c.src, &c.scale_in, &c.out, &c.scale_sum_us, &c.scale_max_us, &c.scale_n, &c.chain_sum_us, &c.chain_max_us, &c.chain_n]
                .map(|a| a.swap(0, Relaxed))
        };
        loop {
            std::thread::sleep(period);
            let Some(p) = weak.upgrade() else { return };
            let state = p.current_state();
            drop(p);
            if state != gst::State::Playing {
                if state == gst::State::Null {
                    return;
                }
                // paused/ready: start a fresh window when it plays again
                all(&c);
                window = std::time::Instant::now();
                continue;
            }
            let secs = window.elapsed().as_secs_f64().max(0.001);
            window = std::time::Instant::now();
            let [src, scale_in, out, s_sum, s_max, s_n, c_sum, c_max, c_n] = all(&c);
            let m = Cadence {
                chain_measured: has_chain,
                cap: cap_fps as f64,
                src: src as f64 / secs,
                out: out as f64 / secs,
                scale_ms: if s_n > 0 { s_sum as f64 / s_n as f64 / 1000.0 } else { 0.0 },
                chain_ms: if c_n > 0 { c_sum as f64 / c_n as f64 / 1000.0 } else { 0.0 },
                chain_busy: c_sum as f64 / 1e6 / secs,
            };
            if !caps_seen {
                if let Some(caps) = src_pad.as_ref().and_then(|w| w.upgrade()).and_then(|p| p.current_caps()) {
                    eprintln!("[lk/cadence] {} source caps: {caps}", utc_hms());
                    caps_seen = true;
                }
            }
            let scaler = if has_scaler {
                format!("scaler {:.1} ({:.1} ms, max {:.1})", scale_in as f64 / secs, m.scale_ms, s_max as f64 / 1000.0)
            } else {
                "scaler n/a".to_string()
            };
            let chain = if has_chain {
                format!("capture chain {:.1} ms/frame (max {:.1}), busy {:.0} %", m.chain_ms, c_max as f64 / 1000.0, m.chain_busy * 100.0)
            } else {
                "capture chain n/a".to_string()
            };
            eprintln!(
                "[lk/cadence] {} {path}: source {:.1} fps -> {scaler} -> appsink {:.1} (cap {cap_fps}); {chain} — {}",
                utc_hms(),
                m.src,
                m.out,
                cadence_verdict(&m)
            );
        }
    });
}

/// One [lk/cadence] window: rates per second, mean milliseconds per frame, and the share of wall time the source's
/// streaming thread spent pushing frames through the synchronous capture chain (0..1).
/// The timing sums, maxima and counts are swapped out one by one, so a frame on a window's edge can land half in
/// each window: the figures are approximate at the edges, which is fine for a 5 s log line.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Cadence {
    /// the source pad and the output queue were both found (else nothing is attributed to the source)
    pub chain_measured: bool,
    pub cap: f64,
    pub src: f64,
    pub out: f64,
    pub scale_ms: f64,
    pub chain_ms: f64,
    pub chain_busy: f64,
}

/// The stage that set a share's rate, in words (the [lk/cadence] line's tail). Order matters: a busy chain explains a
/// low source count (the source can only push the next frame when the chain returns), so it is checked first. The
/// chain time covers the source pad to the output queue only (not allocation before the pad, nor work on frames the
/// rate cap dropped), so a low source rate with a quiet chain is reported as unexplained by it, not proven to be the
/// compositor.
pub(crate) fn cadence_verdict(m: &Cadence) -> &'static str {
    if m.src < 0.5 && m.out < 0.5 {
        "no frames"
    } else if m.out >= m.cap * 0.9 {
        "at the cap (within 10 %)"
    } else if m.chain_busy > 0.7 || (m.scale_ms > 0.0 && m.scale_ms * m.out > 800.0) {
        "the capture pipeline (upload/scale/download) is the ceiling"
    } else if !m.chain_measured {
        "below the cap; this pipeline's stages are not measured"
    } else if m.src < m.cap * 0.9 && m.out >= m.src * 0.9 {
        "few buffers at the source pad, not explained by the measured chain (pipewiresrc/compositor, or work outside it)"
    } else {
        "frames lost between the source and the appsink"
    }
}

/// Wall-clock UTC "HHhMMmSSsZ" for log lines that phases get correlated by (the log has no timestamps otherwise).
fn utc_hms() -> String {
    let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) % 86_400;
    // 12.1.4: "19h47m24sZ", not "19:47:24Z" — the shipped log's IP scrubber (app logship.rs) reads hex groups joined
    // by ':' as an IPv6 address and turned every 12.1.3 stamp into "[ip]24Z".
    format!("{:02}h{:02}m{:02}sZ", s / 3600, s / 60 % 60, s % 60)
}

/// The CPU path's convert + scale: ONE `videoconvertscale` pass (GStreamer 1.22+) where it exists. The old pair
/// converted the WHOLE source to I420 and then read that full-size copy again to scale it; one pass reads the source
/// once and writes only the output. Measured on builder (lanczos, 4 threads, ms per frame): 5120x2160 -> 2560x1072
/// 7.8 -> 5.1, 5120x2160 -> 1920x800 6.5 -> 4.2, 3840x2160 -> 1920x1072 6.0 -> 3.9 — and about 4x all of those while
/// another build loaded the box, i.e. what a game does to the share on the same machine.
fn cpu_convert_scale(threads: u32) -> String {
    static ONE_PASS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let one_pass = *ONE_PASS.get_or_init(|| {
        let _ = gst::init();
        gst::ElementFactory::find("videoconvertscale").is_some()
    });
    if one_pass {
        format!("videoconvertscale method=lanczos add-borders=false n-threads={threads} name=vscale")
    } else {
        format!("videoconvert n-threads={threads} ! videoscale method=lanczos add-borders=false n-threads={threads} name=vscale")
    }
}

/// The launch line of the CPU capture (tests feed a videotestsrc head).
pub(crate) fn cpu_capture_desc(src: &str, download: &str, fps: u32, threads: u32) -> String {
    let convert_scale = cpu_convert_scale(threads);
    format!(
        "{src} ! \
         videorate drop-only=true ! \
         video/x-raw(ANY),framerate={fps}/1 ! \
         {download}{convert_scale} ! \
         capsfilter name=outcaps caps=video/x-raw,format=I420 ! \
         queue name=outq max-size-buffers=4 leaky=downstream ! \
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
    // the capture -> wall mapping (re-bases on a backwards or far jump: see CaptureClock)
    let clock: std::sync::Arc<std::sync::Mutex<CaptureClock>> = Default::default();
    // Rolling evidence for the next "it is still jittery" report: how far apart the
    // CAPTURE clock and the ARRIVAL clock drift. Big numbers here mean the capture chain
    // is being starved, and say so without needing a profiler on the user's machine.
    let jitter_sum_us = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0));
    let jitter_max_us = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0));
    let jitter_n = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0));
    // wall µs of the last [lk/pacing] report, for the delivered rate
    let report_at = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0));
    let prev_arrival_us = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0));
    let prev_pts_us = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0));
    let (clock_cb, clock_rep) = (clock.clone(), clock.clone());
    let (jsum_cb, jmax_cb, jn_cb, report_cb) = (jitter_sum_us.clone(), jitter_max_us.clone(), jitter_n.clone(), report_at.clone());
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

                // Capture-clock timestamp (see tinhat-capture-pts-v1 above and CaptureClock). Falls back to
                // 0 (= SDK stamps on arrival) when the source gives us no PTS. Arrival is on a monotonic clock.
                let now_us = mono_epoch_us();
                // u64 ns -> i64 µs without a wrap (u64::MAX ns / 1000 fits an i64)
                let pts_us = buffer.pts().map(|t| (t.nseconds() / 1000) as i64);
                // Stamp AND submit under the clock's lock: the keepalive repeater stamps from the same clock, and
                // the order libwebrtc sees must be the order the stamps were handed out.
                let mut clock = clock_cb.lock().unwrap_or_else(|e| e.into_inner());
                let (stamp_us, rebased) = match pts_us {
                    _ if arrival_ts_mode => (0, None),
                    Some(pts) => clock.stamp(pts, now_us),
                    // no PTS: still this clock's timeline (the SDK's own clock would be a second one)
                    None => (clock.stamp_no_pts(now_us), None),
                };
                if let Some(jump_ms) = rebased {
                    // said at once (the 150-frame report would hide WHEN); a storm is summarised there
                    if clock.stats.rebased_back + clock.stats.rebased_jump <= 3 {
                        eprintln!(
                            "[lk/pacing] capture PTS {} by {} ms between two frames — re-based the frame timeline (no frame dropped)",
                            if jump_ms < 0 { "went back" } else { "jumped ahead" },
                            jump_ms.abs()
                        );
                    }
                }
                if let (Some(pts), false) = (pts_us, arrival_ts_mode) {
                    // Measure how badly delivery is clumping vs the capture cadence (signed; a pair across a
                    // re-base or a reset is not a skew sample).
                    let pa = parr_cb.swap(now_us, std::sync::atomic::Ordering::Relaxed);
                    let pp = ppts_cb.swap(pts, std::sync::atomic::Ordering::Relaxed);
                    if pa != 0 && rebased.is_none() {
                        if let Some(skew) = delivery_skew_us(pa, now_us, pp, pts) {
                            jsum_cb.fetch_add(skew, std::sync::atomic::Ordering::Relaxed);
                            jmax_cb.fetch_max(skew, std::sync::atomic::Ordering::Relaxed);
                            jn_cb.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                }
                let report = if n > 0 && n % 150 == 0 { Some(clock.take_stats()) } else { None };
                let frame = VideoFrame {
                    rotation: VideoRotation::VideoRotation0,
                    timestamp_us: stamp_us,
                    frame_metadata: None,
                    buffer: dst,
                };
                source.capture_frame(&frame);
                drop(clock);
                if let Some(st) = report {
                    let sum = jsum_cb.swap(0, std::sync::atomic::Ordering::Relaxed);
                    let max = jmax_cb.swap(0, std::sync::atomic::Ordering::Relaxed);
                    let cnt = jn_cb.swap(0, std::sync::atomic::Ordering::Relaxed);
                    let since = report_cb.swap(now_us, std::sync::atomic::Ordering::Relaxed);
                    let rate = if since > 0 && now_us > since { format!("{:.1} fps", 150.0 * 1e6 / (now_us - since) as f64) } else { "-".into() };
                    let fixes = if st == ClockStats::default() {
                        "timeline clean".to_string()
                    } else {
                        format!(
                            "timeline: re-based {} (PTS went back {}, jumped ahead {}; last {} ms), re-anchored {}, nudged {}, keepalive repeats {}, no PTS {}",
                            st.rebased_back + st.rebased_jump,
                            st.rebased_back,
                            st.rebased_jump,
                            st.last_jump_ms.map(|j| j.to_string()).unwrap_or_else(|| "-".into()),
                            st.reanchored,
                            st.nudged,
                            st.repeats,
                            st.no_pts
                        )
                    };
                    eprintln!(
                        "[lk/pacing] last 150 frames: mean delivery-vs-capture skew {:.1}ms, worst {:.1}ms; {rate} to the encoder; {fixes}",
                        if cnt > 0 { sum as f64 / cnt as f64 / 1000.0 } else { 0.0 },
                        max as f64 / 1000.0
                    );
                }
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
            // 12.1.4: stamped by the feeder's own clock (never before the last capture stamp) and submitted under
            // its lock, so a repeat can't land out of order with a resuming capture. The SDK's own clock (stamp 0)
            // is a different timeline. Arrival-time mode keeps 0, like its captures.
            let mut clock = clock_rep.lock().unwrap_or_else(|e| e.into_inner());
            let ts = if arrival_ts_mode { 0 } else { clock.stamp_repeat(mono_epoch_us()) };
            let frame = VideoFrame {
                rotation: VideoRotation::VideoRotation0,
                timestamp_us: ts,
                frame_metadata: None,
                buffer: dst,
            };
            source_rep.capture_frame(&frame);
            drop(clock);
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

/// The capture clock of a share/camera feeder: maps a capture PTS onto the epoch WebRTC expects (micros since the UNIX
/// epoch) so the SPACING comes from the capture timeline (which `videorate` has already snapped to a fixed-fps grid)
/// while the EPOCH comes from the clock. Delivery jitter (frames landing in clumps because a game is monopolising the
/// GPU) then no longer reaches the encoder's timeline. See `tinhat-capture-pts-v1`.
///
/// 12.1.4 (BUG-20261030 finding 1): it RE-BASES instead of clinging to a broken mapping. 12.1.3 kept the result
/// strictly increasing by stamping `prev + 1 µs` whenever the mapping said "earlier". After a capture hiccup one frame
/// carried a wrapped PTS (a slightly negative running time read as u64 nanoseconds: ~584 years ahead); it became
/// `prev`, and every later frame was stamped 1 µs after it, for the rest of the session; libwebrtc then encoded about
/// half the frames: "the share settles back to ~30 fps".
///
/// A DISCONTINUITY is judged from the PTS step against the arrival step, both from this feeder (arrival = a monotonic
/// clock anchored once to the epoch, so wall-clock steps and drift can't trigger it): the PTS went back by more than
/// `BACK_TOL_US`, or moved forward `JUMP_US` more than real time did. A discontinuity starts a new mapping one frame
/// step after the previous stamp. A frame that merely maps to before the previous stamp (keepalive repeats or a
/// drained backlog ran ahead) is re-anchored after it once (more than `BACK_TOL_US` behind) or nudged by 1 µs
/// (within it). Nothing is dropped; every correction is counted by reason. Stamps never decrease, including the
/// keepalive's ([`Self::stamp_repeat`]).
#[derive(Debug, Default)]
pub(crate) struct CaptureClock {
    /// (capture PTS µs, stamp µs) the mapping is anchored at
    base: Option<(i64, i64)>,
    /// the previous capture frame's (PTS µs, arrival µs)
    prev: Option<(i64, i64)>,
    /// the last stamp handed out (µs since the epoch); 0 = none yet
    last: i64,
    /// the last plausible stamp-to-stamp spacing (µs), the step a re-base continues with
    step: i64,
    pub stats: ClockStats,
}

/// What the capture clock had to correct since the last report (see [`CaptureClock::take_stats`]).
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct ClockStats {
    /// re-based: the capture PTS went back (a capture restart or a reset clock)
    pub rebased_back: u64,
    /// re-based: the capture PTS jumped forward far more than time passed (a wrapped PTS, a clock change)
    pub rebased_jump: u64,
    /// re-anchored: the frame mapped to well before the previous stamp (repeats or a backlog ran ahead)
    pub reanchored: u64,
    /// a tiny regression (within the tolerance) nudged to 1 µs after the previous stamp
    pub nudged: u64,
    /// keepalive repeats stamped
    pub repeats: u64,
    /// capture frames that came without a PTS (stamped on arrival, by this clock)
    pub no_pts: u64,
    /// the PTS step of the last re-base in ms (signed: negative = went back)
    pub last_jump_ms: Option<i64>,
}

impl CaptureClock {
    /// The PTS may run ahead of real time by this much between two frames before it counts as a jump.
    pub(crate) const JUMP_US: i64 = 1_000_000;
    /// A backwards step up to this much is jitter; beyond it the capture clock restarted.
    pub(crate) const BACK_TOL_US: i64 = 50_000;
    const DEFAULT_STEP_US: i64 = 16_667;

    /// The stamp for a capture frame with PTS `pts_us` that arrived at `now_us` (monotonic, epoch-anchored); and the
    /// PTS step in ms when this frame re-based the mapping.
    pub(crate) fn stamp(&mut self, pts_us: i64, now_us: i64) -> (i64, Option<i64>) {
        #[cfg(test)]
        if LEGACY_RULE_FOR_LAB.load(std::sync::atomic::Ordering::SeqCst) {
            // 12.1.3's rule, for the lab's side-by-side run only
            let (p0, w0) = *self.base.get_or_insert((pts_us, now_us));
            let want = w0.saturating_add(pts_us.saturating_sub(p0));
            let ts = if want <= self.last { self.last.saturating_add(1) } else { want };
            self.last = ts;
            return (ts, None);
        }
        let prev = self.prev.replace((pts_us, now_us));
        let Some((base_pts, base_ts)) = self.base else {
            self.base = Some((pts_us, now_us.max(self.next_after_last())));
            let ts = self.base.unwrap().1;
            return (self.advance(ts), None);
        };
        if let Some((ppts, parr)) = prev {
            let dp = pts_us.saturating_sub(ppts);
            let da = now_us.saturating_sub(parr).max(0);
            if dp < -Self::BACK_TOL_US || dp > da.saturating_add(Self::JUMP_US) {
                if dp < 0 {
                    self.stats.rebased_back += 1;
                } else {
                    self.stats.rebased_jump += 1;
                }
                let jump_ms = dp / 1000;
                self.stats.last_jump_ms = Some(jump_ms);
                let ts = now_us.max(self.next_after_last());
                self.base = Some((pts_us, ts));
                return (self.advance(ts), Some(jump_ms));
            }
        }
        let want = base_ts.saturating_add(pts_us.saturating_sub(base_pts));
        if self.last != 0 && want <= self.last {
            if self.last.saturating_sub(want) > Self::BACK_TOL_US {
                self.stats.reanchored += 1;
                let ts = self.next_after_last();
                self.base = Some((pts_us, ts));
                return (self.advance(ts), None);
            }
            self.stats.nudged += 1;
            return (self.advance(self.last.saturating_add(1)), None);
        }
        (self.advance(want), None)
    }

    /// The stamp for a keepalive repeat at `now_us`: the clock's time, never before the previous stamp.
    pub(crate) fn stamp_repeat(&mut self, now_us: i64) -> i64 {
        self.stats.repeats += 1;
        self.stamp_arrival(now_us)
    }

    /// The stamp for a capture frame that came without a PTS: its arrival, never before the previous stamp.
    pub(crate) fn stamp_no_pts(&mut self, now_us: i64) -> i64 {
        self.stats.no_pts += 1;
        self.stamp_arrival(now_us)
    }

    fn stamp_arrival(&mut self, now_us: i64) -> i64 {
        let ts = now_us.max(self.last.saturating_add(1_000));
        self.last = ts;
        ts
    }

    fn next_after_last(&self) -> i64 {
        if self.last == 0 {
            return 0;
        }
        self.last.saturating_add(if self.step > 0 { self.step } else { Self::DEFAULT_STEP_US })
    }

    fn advance(&mut self, ts: i64) -> i64 {
        if self.last != 0 {
            let d = ts.saturating_sub(self.last);
            if (1_000..=100_000).contains(&d) {
                self.step = d;
            }
        }
        self.last = ts;
        ts
    }

    /// The counters since the last call (they reset).
    pub(crate) fn take_stats(&mut self) -> ClockStats {
        std::mem::take(&mut self.stats)
    }
}

/// The lab case runs 12.1.3's stamping rule through the same feeder and SDK path for comparison.
#[cfg(test)]
pub(crate) static LEGACY_RULE_FOR_LAB: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Micros since the UNIX epoch on a MONOTONIC clock: the epoch read once per process, then `Instant` (a wall-clock
/// step, e.g. NTP, can't move capture stamps).
pub(crate) fn mono_epoch_us() -> i64 {
    static ANCHOR: std::sync::OnceLock<(std::time::Instant, i64)> = std::sync::OnceLock::new();
    let (t0, e0) = *ANCHOR.get_or_init(|| {
        let e = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_micros() as i64).unwrap_or(0);
        (std::time::Instant::now(), e)
    });
    e0.saturating_add(t0.elapsed().as_micros() as i64)
}

/// The pre-12.1.4 helper's shape (kept for the tests that describe the mapping): one shared clock behind a mutex.
#[cfg(test)]
fn rebase_capture_ts(pts_us: i64, now_us: i64, clock: &std::sync::Mutex<CaptureClock>) -> i64 {
    clock.lock().unwrap().stamp(pts_us, now_us).0
}

/// One delivery-vs-capture skew sample: how much more (or less) wall time passed between two deliveries than capture
/// time between their PTS. Signed maths throughout; None when the pair spans a re-base or the PTS step is not a
/// plausible frame step (a reset or a wrapped PTS would otherwise print a nonsense skew).
pub(crate) fn delivery_skew_us(prev_arrival: i64, arrival: i64, prev_pts: i64, pts: i64) -> Option<i64> {
    let dp = pts.checked_sub(prev_pts)?;
    let da = arrival.checked_sub(prev_arrival)?;
    if !(0..=CaptureClock::JUMP_US).contains(&dp) || !(0..=CaptureClock::JUMP_US).contains(&da) {
        return None;
    }
    Some((da - dp).abs())
}

#[cfg(test)]
mod pacing_tests {
    use super::{delivery_skew_us, rebase_capture_ts, CaptureClock, ClockStats};
    use std::sync::Mutex;

    /// The reported bug in miniature: a 30fps capture whose frames are DELIVERED in
    /// clumps (the GPU is busy) must still leave the encoder on an even 33.3ms cadence.
    /// Stamping arrival times — what `timestamp_us: 0` does — reproduces the clumping,
    /// which is the judder.
    #[test]
    fn clumped_delivery_still_yields_an_even_capture_cadence() {
        let clock = Mutex::new(CaptureClock::default());
        const STEP: i64 = 33_333; // 30fps
        let wall0 = 1_700_000_000_000_000i64;

        // Capture is even; delivery is not: three frames land almost together, then a
        // long stall, repeatedly. This is what a GPU-bound game does to the portal.
        let delivery_offsets = [0i64, 1_000, 2_000, 130_000, 131_000, 132_000, 260_000];
        let mut out = Vec::new();
        for (i, off) in delivery_offsets.iter().enumerate() {
            let pts = i as i64 * STEP;
            out.push(rebase_capture_ts(pts, wall0 + off, &clock));
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
        let clock = Mutex::new(CaptureClock::default());
        let wall0 = 1_700_000_000_000_000i64;
        let a = rebase_capture_ts(100_000, wall0, &clock);
        let b = rebase_capture_ts(133_333, wall0 + 33_000, &clock);
        let c = rebase_capture_ts(0, wall0 + 66_000, &clock); // PTS jumped back
        let d = rebase_capture_ts(33_333, wall0 + 99_000, &clock);
        assert!(b > a, "normal progression");
        assert!(c > b, "a backwards PTS must still move the stamp forward");
        assert!(d > c, "and recovery must keep moving forward");
        // 12.1.4: and recovery keeps the real spacing, not 1 µs steps
        assert_eq!(d - c, 33_333, "after the jump back the capture spacing continues");
    }

    const F60: i64 = 16_667;
    const WALL0: i64 = 1_790_000_000_000_000;
    /// A wrapped PTS: a running time of -9.3 s read as u64 nanoseconds, in µs (a 12.1.3 field log: the skew line
    /// printed 18446734725993 ms).
    const WRAPPED_US: i64 = ((u64::MAX - 9_300_000_000) / 1000) as i64;

    /// What 12.1.3 did: never go backwards by stamping prev + 1 µs (kept here to show the failure).
    fn old_rule(pts: i64, now: i64, base: &mut Option<(i64, i64)>, last: &mut i64) -> i64 {
        let (p0, w0) = *base.get_or_insert((pts, now));
        let want = w0 + (pts - p0);
        let ts = if want <= *last { *last + 1 } else { want };
        *last = ts;
        ts
    }

    /// BUG-20261030 finding 1 in miniature: 60 fps, ONE frame with a wrapped PTS, then normal frames again.
    #[test]
    fn a_wrapped_pts_does_not_poison_the_timeline() {
        let frames: Vec<(i64, i64)> = (0..240i64)
            .map(|i| {
                let pts = if i == 120 { WRAPPED_US } else { i * F60 };
                (pts, WALL0 + i * F60 + 300) // delivered 0.3 ms after capture
            })
            .collect();
        let mut clock = CaptureClock::default();
        let new: Vec<i64> = frames.iter().map(|&(p, w)| clock.stamp(p, w).0).collect();
        let (mut base, mut last) = (None, 0i64);
        let old: Vec<i64> = frames.iter().map(|&(p, w)| old_rule(p, w, &mut base, &mut last)).collect();

        assert!(new.windows(2).all(|w| w[1] > w[0]), "strictly increasing");
        // after the bad frame: real 60 fps spacing again, and the stamps stay near the wall clock
        for w in new[122..].windows(2) {
            assert_eq!(w[1] - w[0], F60);
        }
        let (_, wall_last) = frames[239];
        assert!((new[239] - wall_last).abs() < 100_000, "stamp {} vs wall {wall_last}", new[239]);
        let st = clock.take_stats();
        assert_eq!((st.rebased_jump, st.rebased_back), (1, 1), "{st:?}");
        // 12.1.3's rule: 1 µs apart for the rest of the session (what libwebrtc's frame-rate limiter then halved)
        assert!(old[122..].windows(2).all(|w| w[1] - w[0] == 1), "the old rule stamped 1 µs steps");
        assert!(old[239] - wall_last > 1_000_000_000_000, "and centuries ahead of the wall clock");
    }

    /// A PTS reset (the capture restarted its clock at 0) mid-share: one re-base, spacing kept.
    #[test]
    fn a_pts_reset_rebases_once_and_keeps_the_spacing() {
        let mut clock = CaptureClock::default();
        let mut out = Vec::new();
        for i in 0..200i64 {
            let pts = if i < 100 { 5_000_000 + i * F60 } else { (i - 100) * F60 };
            out.push(clock.stamp(pts, WALL0 + i * F60).0);
        }
        assert!(out.windows(2).all(|w| w[1] - w[0] == F60), "even spacing straight through the reset");
        let st = clock.take_stats();
        assert_eq!((st.rebased_back, st.rebased_jump, st.nudged), (1, 0, 0), "{st:?}");
        assert_eq!(st.last_jump_ms, Some(-6_650)); // 5 s of PTS + the 99 frames already mapped
        assert_eq!(clock.take_stats(), ClockStats::default(), "counters reset after a report");
    }

    /// Jitter backwards within the tolerance is nudged, not re-based; a far forward jump re-bases.
    #[test]
    fn small_regressions_nudge_far_jumps_rebase() {
        let mut clock = CaptureClock::default();
        let a = clock.stamp(1_000_000, WALL0).0;
        let b = clock.stamp(1_000_000 - 10_000, WALL0 + F60).0; // 10 ms back: jitter
        assert_eq!(b, a + 1);
        let c = clock.stamp(1_000_000 + 5_000_000, WALL0 + 2 * F60).0; // 5 s ahead of the wall clock
        assert!(c > b && c - b < 100_000, "re-based next to the previous stamp, not 5 s ahead");
        let st = clock.take_stats();
        assert_eq!((st.nudged, st.rebased_jump, st.rebased_back), (1, 1, 0), "{st:?}");
    }

    /// Astra (12.1.4 review): a drained backlog (frames delivered 2 s late, then in a burst) must not re-base over and
    /// over; it re-anchors at most once and the spacing stays the capture spacing.
    #[test]
    fn a_drained_backlog_does_not_rebase_in_a_loop() {
        let mut clock = CaptureClock::default();
        let mut out = Vec::new();
        for i in 0..300i64 {
            // frames 100..220 arrive in a burst 2 s late, 1 ms apart; the rest on time
            let now = if (100..220).contains(&i) { WALL0 + 100 * F60 + 2_000_000 + (i - 100) * 1_000 } else { WALL0 + i * F60 + 2_000_000 };
            out.push(clock.stamp(i * F60, now).0);
        }
        assert!(out.windows(2).all(|w| w[1] > w[0]));
        let st = clock.take_stats();
        assert_eq!(st.rebased_back + st.rebased_jump, 0, "{st:?}");
        assert!(st.reanchored <= 1, "{st:?}");
        assert_eq!(out[299] - out[298], F60);
    }

    /// Keepalive repeats come from the same clock: never before the last capture stamp, and a capture that resumes
    /// after them stays after them.
    #[test]
    fn repeats_and_resumed_captures_stay_in_order() {
        let mut clock = CaptureClock::default();
        let a = clock.stamp(0, WALL0).0;
        let b = clock.stamp(F60, WALL0 + F60).0;
        let r1 = clock.stamp_repeat(WALL0 + 300_000);
        let r2 = clock.stamp_repeat(WALL0 + 400_000);
        // the screen moves again: its PTS (running time) advanced with real time
        let c = clock.stamp(450_000, WALL0 + 450_000).0;
        let d = clock.stamp(450_000 + F60, WALL0 + 450_000 + F60).0;
        assert!(a < b && b < r1 && r1 < r2 && r2 < c && c < d, "{a} {b} {r1} {r2} {c} {d}");
        assert_eq!(d - c, F60);
        assert_eq!(clock.take_stats().repeats, 2);
    }

    /// The skew sample is signed maths and skips pairs that are not a frame step.
    #[test]
    fn skew_is_signed_and_skips_resets() {
        assert_eq!(delivery_skew_us(0, 20_000, 0, F60), Some(20_000 - F60));
        assert_eq!(delivery_skew_us(0, 10_000, 0, F60), Some(F60 - 10_000));
        assert_eq!(delivery_skew_us(0, F60, 5_000_000, 0), None, "PTS went back");
        assert_eq!(delivery_skew_us(0, F60, 0, WRAPPED_US), None, "wrapped PTS");
        assert_eq!(delivery_skew_us(i64::MIN, 0, 0, F60), None, "no overflow");
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

    /// The share size rule, case by case from the vectors Tin Hat Web runs too (testvectors/share-fit).
    #[test]
    fn share_fit_matches_the_shared_vectors() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testvectors/share-fit/vectors.json");
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let cases = v["cases"].as_array().unwrap();
        assert!(cases.len() >= 10);
        let pair = |c: &serde_json::Value, k: &str| (c[k][0].as_u64().unwrap() as u32, c[k][1].as_u64().unwrap() as u32);
        for c in cases {
            let (sw, sh) = pair(c, "src");
            let (bw, bh) = pair(c, "box");
            assert_eq!(fit_even(sw, sh, bw, bh), pair(c, "out"), "{}", c["name"]);
        }
    }

    /// A 21:9 5120x2160 under every share preset: the preset's lines, the screen's aspect, H.264-safe.
    #[test]
    fn ultrawide_keeps_the_preset_lines() {
        for (bw, bh) in [(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)] {
            let (w, h) = fit_even(5120, 2160, bw, bh);
            assert!(w <= SHARE_MAX_DIM && h <= SHARE_MAX_DIM && w % 16 == 0 && h % 16 == 0, "{w}x{h}");
            if 5120 * bh / 2160 <= SHARE_MAX_DIM {
                assert!(h + 16 > bh, "{bw}x{bh} -> {w}x{h} lost lines");
            } else {
                assert_eq!(w, SHARE_MAX_DIM, "{bw}x{bh} -> {w}x{h}: the long side is the limit");
            }
            let aspect = w as f64 / h as f64;
            assert!((aspect - 5120.0 / 2160.0).abs() < 0.03, "{bw}x{bh} -> {w}x{h} aspect {aspect}");
        }
    }

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

    /// No NVIDIA (no CUDA filters, bundled NVRTC or not) → the CPU path on Linux whatever the size; with CUDA → GPU.
    #[test]
    fn share_capture_plan_without_nvidia_is_the_cpu_path() {
        // Linux, no CUDA filters: neither GPU variant, size known or not
        assert_eq!(share_capture_plan(true, true, false, false, false, false), (false, false));
        assert_eq!(share_capture_plan(false, true, false, false, false, false), (false, false));
        // Linux with CUDA: GPU when the size is known, GPU-learn when not
        assert_eq!(share_capture_plan(true, true, true, false, false, false), (true, false));
        assert_eq!(share_capture_plan(false, true, true, false, false, false), (false, true));
        // TINHAT_CAPTURE_CPU wins
        assert_eq!(share_capture_plan(true, true, true, false, true, false), (false, false));
        // Windows: D3D11 alone is a GPU path; never the learn variant; the d3d11 flag means nothing on Linux
        assert_eq!(share_capture_plan(true, true, false, true, false, true), (true, false));
        assert_eq!(share_capture_plan(true, true, false, true, false, false), (false, false));
        assert_eq!(share_capture_plan(true, false, true, true, false, true), (false, false));
    }

    /// RC2: a first start that learned the size builds its pipeline with fit_even(learned, box) — the very dims a
    /// re-publish gets from publish_target(Some(learned)), so the two starts run the same launch line.
    #[test]
    fn first_start_dims_equal_republish_dims() {
        for (bw, bh) in [(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)] {
            let q = ShareQuality { max_w: bw, max_h: bh, ..ShareQuality::default() };
            for src in [(5120, 2160), (3840, 2160), (3233, 2150), (2560, 1440), (1920, 1080)] {
                let (w, h, _, _) = publish_target(Some(src), q);
                assert_eq!(fit_even(src.0, src.1, share_box(q).0, share_box(q).1), (w, h), "{src:?} in {bw}x{bh}");
            }
        }
    }

    /// RC2 share-START gate (the 12.1.2 RC1 field diagnosis, F2): under load, a share started cold with no known size
    /// must run like a re-publish. Cold starts from a live 5120x2160 BGRA source at 90 fps (KWin's shm frames
    /// stand-in, PTS restamped on arrival like pipewiresrc's), 1440p box, cap 60, each measured at the appsink over a
    /// fixed window after a 3 s warm-up, in the order C, B, C, A:
    ///   C = the re-publish (the pre-sized pipeline straight away)
    ///   B = RC2's first start (learn the size, stop and drop that pipeline, start the pre-sized one)
    ///   A = RC1's first start (the first-frame "learn" pipeline kept as the share; reported, not asserted)
    /// Pass: every run got frames with the expected output caps and no bus error, B >= 90 % of mean(C), and in CUDA
    /// mode B and C each >= 90 % of the cap (the 60 fps claim). CUDA mode = the real launch lines + parallel 8K CUDA
    /// scale pipelines as GPU load; elsewhere the software stand-in under one spinning thread per core. This is a
    /// pipeline-level gate: the portal, start_share_capture's candidate handling and a cold process are the release candidate's field test.
    /// Opt-in: TINHAT_SHARE_START_GATE=1 [TINHAT_GATE_SECS=15] [TINHAT_GATE_GPU_LOAD=4] [TINHAT_GATE_REQUIRE_CUDA=1].
    #[test]
    fn share_start_gate() {
        if std::env::var_os("TINHAT_SHARE_START_GATE").is_none() {
            eprintln!("skipping: TINHAT_SHARE_START_GATE not set");
            return;
        }
        let _ = gst::init();
        let secs: f64 = std::env::var("TINHAT_GATE_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(15.0);
        assert!(secs.is_finite() && (3.0..=600.0).contains(&secs), "TINHAT_GATE_SECS out of range: {secs}");
        let gpu_load_n: usize = std::env::var("TINHAT_GATE_GPU_LOAD").ok().map(|v| v.parse().expect("TINHAT_GATE_GPU_LOAD")).unwrap_or(4);
        assert!((1..=32).contains(&gpu_load_n), "TINHAT_GATE_GPU_LOAD out of range");
        let require_cuda = std::env::var_os("TINHAT_GATE_REQUIRE_CUDA").is_some();

        // a pipeline that must stay healthy: Err on a bus error
        let bus_error = |p: &gst::Pipeline| -> Option<String> {
            let msg = p.bus()?.pop_filtered(&[gst::MessageType::Error])?;
            match msg.view() {
                gst::MessageView::Error(e) => Some(format!("{} ({:?})", e.error(), e.debug())),
                _ => None,
            }
        };
        let launch = |d: &str| gst::parse::launch(d).unwrap_or_else(|e| panic!("{e}: {d}")).downcast::<gst::Pipeline>().unwrap();

        // CUDA mode only when a CUDA chain really RUNS here: a registry scanned on another machine can list the
        // elements without a GPU (a build container without a GPU).
        let cuda = cuda_scale_available()
            && gst::parse::launch(
                "videotestsrc num-buffers=1 ! video/x-raw,format=BGRA,width=64,height=64 ! cudaupload ! cudaconvertscale ! \
                 video/x-raw(memory:CUDAMemory),format=I420,width=32,height=32 ! cudadownload ! fakesink",
            )
            .ok()
            .and_then(|p| p.downcast::<gst::Pipeline>().ok())
            .is_some_and(|p| {
                let ok = p.set_state(gst::State::Playing).is_ok()
                    && p.bus()
                        .and_then(|b| b.timed_pop_filtered(gst::ClockTime::from_seconds(3), &[gst::MessageType::Eos, gst::MessageType::Error]))
                        .is_some_and(|m| matches!(m.view(), gst::MessageView::Eos(_)));
                let _ = p.set_state(gst::State::Null);
                ok
            });
        assert!(cuda || !require_cuda, "TINHAT_GATE_REQUIRE_CUDA: no working CUDA chain here");

        let src = "videotestsrc name=gatesrc is-live=true pattern=solid-color ! video/x-raw,format=BGRA,width=5120,height=2160,framerate=90/1";
        let (box_w, box_h, fps) = (2560u32, 1440u32, 60u32);
        let emulate = |d: String| -> String {
            if cuda {
                d
            } else {
                d.replace("cudaupload ! cudaconvertscale name=vscale", &format!("videoconvertscale method=lanczos add-borders=false n-threads={} name=vscale", cpu_capture_threads()))
                    .replace("(memory:CUDAMemory)", "")
                    .replace("cudadownload ! ", "")
            }
        };
        let feature = if cuda { Some("memory:CUDAMemory") } else { None };
        let learn_desc = emulate(cuda_learn_desc(src, fps));
        let sink_of = |p: &gst::Pipeline| p.by_name("sink").unwrap().downcast::<gstreamer_app::AppSink>().unwrap();
        // pipewiresrc stamps buffers on arrival; videotestsrc keeps its nominal 90 fps stamps even when it falls
        // behind, and the rate cap would then drop a third of what arrives. Restamp PTS to the running time.
        let restamp = |p: &gst::Pipeline| {
            let pad = p.by_name("gatesrc").and_then(|s| s.static_pad("src")).unwrap();
            let weak = p.downgrade();
            pad.add_probe(gst::PadProbeType::BUFFER, move |_, info| {
                let now = weak.upgrade().and_then(|p| p.current_running_time());
                if let (Some(now), Some(gst::PadProbeData::Buffer(buf))) = (now, info.data.as_mut()) {
                    buf.make_mut().set_pts(now);
                }
                gst::PadProbeReturn::Ok
            });
        };
        // (fps over the fixed window, the first sample's size)
        let measure = |p: gst::Pipeline, label: &'static str| -> (f64, Option<(u32, u32)>) {
            restamp(&p);
            install_cadence_probes(&p, fps, label);
            let sink = sink_of(&p);
            p.set_state(gst::State::Playing).unwrap();
            let t0 = std::time::Instant::now();
            let (mut n, mut size) = (0u32, None);
            while t0.elapsed().as_secs_f64() < 3.0 + secs {
                if let Some(sample) = sink.try_pull_sample(gst::ClockTime::from_mseconds(200)) {
                    if size.is_none() {
                        size = sample.caps().and_then(|c| gstreamer_video::VideoInfo::from_caps(c).ok()).map(|i| (i.width(), i.height()));
                    }
                    if t0.elapsed().as_secs_f64() >= 3.0 {
                        n += 1;
                    }
                }
                if let Some(e) = bus_error(&p) {
                    panic!("{label}: bus error {e}");
                }
            }
            let _ = p.set_state(gst::State::Null);
            (n as f64 / secs, size)
        };

        // the load, stopped by a guard (also on a panic)
        struct Load {
            stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
            threads: Vec<std::thread::JoinHandle<()>>,
            pipelines: Vec<gst::Pipeline>,
        }
        impl Drop for Load {
            fn drop(&mut self) {
                self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
                for t in self.threads.drain(..) {
                    let _ = t.join();
                }
                for p in &self.pipelines {
                    let _ = p.set_state(gst::State::Null);
                }
            }
        }
        let mut load = Load { stop: Default::default(), threads: Vec::new(), pipelines: Vec::new() };
        if cuda {
            for _ in 0..gpu_load_n {
                let p = launch(
                    "videotestsrc pattern=solid-color ! video/x-raw,format=BGRA,width=7680,height=4320 ! cudaupload ! \
                     cudaconvertscale ! video/x-raw(memory:CUDAMemory),format=I420,width=3840,height=2160 ! fakesink sync=false",
                );
                p.set_state(gst::State::Playing).unwrap();
                load.pipelines.push(p);
            }
        } else {
            for _ in 0..std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4) {
                let stop = load.stop.clone();
                load.threads.push(std::thread::spawn(move || {
                    let mut x = 0u64;
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
                    }
                }));
            }
        }
        // the load must hold for every measured run, not just at the start
        let load_ok = |load: &Load, when: &str| {
            for p in &load.pipelines {
                if let Some(e) = bus_error(p) {
                    panic!("the GPU load pipeline failed {when}: {e}");
                }
                assert_eq!(p.current_state(), gst::State::Playing, "the GPU load pipeline stopped {when}");
            }
        };
        std::thread::sleep(std::time::Duration::from_secs(1));
        load_ok(&load, "before the runs");

        let presized = |(rw, rh): (u32, u32)| {
            let (ow, oh) = fit_even(rw, rh, box_w, box_h);
            launch(&emulate(cuda_presized_desc(src, ow, oh, fps)))
        };
        let (fps_c1, size_c1) = measure(presized((5120, 2160)), "gate C re-publish");
        load_ok(&load, "during C1");

        // B: RC2's first start: learn (first frame), stop AND drop the learner, then the pre-sized pipeline
        let t0 = std::time::Instant::now();
        let learned = {
            let memo: std::sync::Arc<std::sync::Mutex<Option<(u32, u32)>>> = Default::default();
            let learner = launch(&learn_desc);
            restamp(&learner);
            install_first_frame_repin(&learner, box_w, box_h, feature, memo.clone(), "gate");
            let sink = sink_of(&learner);
            learner.set_state(gst::State::Playing).unwrap();
            while sink.try_pull_sample(gst::ClockTime::from_mseconds(200)).is_none() {
                assert!(t0.elapsed().as_secs() < 4, "the learning pipeline gave no frame in 4 s");
                assert!(bus_error(&learner).is_none(), "the learning pipeline failed");
            }
            let _ = learner.set_state(gst::State::Null);
            let learned = memo.lock().unwrap().expect("the first frame carried no size");
            learned
        };
        let learn_ms = t0.elapsed().as_millis();
        let (fps_b, size_b) = measure(presized(learned), "gate B rc2-first-start");
        load_ok(&load, "during B");

        let (fps_c2, size_c2) = measure(presized((5120, 2160)), "gate C re-publish");
        load_ok(&load, "during C2");
        let fps_c = (fps_c1 + fps_c2) / 2.0;

        let a = launch(&learn_desc);
        install_first_frame_repin(&a, box_w, box_h, feature, Default::default(), "gate");
        let (fps_a, _) = measure(a, "gate A rc1-first-start");
        load_ok(&load, "during A");
        drop(load);

        let mode = if cuda { "GPU (CUDA)" } else { "CPU (software stand-in, no CUDA here)" };
        eprintln!(
            "[gate] share start under {mode} load: A rc1-first-start {fps_a:.1} fps | B rc2-first-start {fps_b:.1} fps \
             (learned {}x{} in {learn_ms} ms) | C re-publish {fps_c:.1} fps ({fps_c1:.1} / {fps_c2:.1}) (cap {fps}, {secs} s windows)",
            learned.0, learned.1
        );
        assert_eq!(learned, (5120, 2160));
        for (label, size) in [("B", size_b), ("C1", size_c1), ("C2", size_c2)] {
            assert_eq!(size, Some((3408, 1440)), "{label}: output caps");
        }
        assert!(fps_b > 0.0 && fps_c1 > 0.0 && fps_c2 > 0.0, "a run got no frames");
        assert!(fps_b >= fps_c * 0.9, "first start {fps_b:.1} fps vs re-publish {fps_c:.1} fps");
        if cuda {
            let floor = fps as f64 * 0.9;
            assert!(fps_b >= floor && fps_c >= floor, "the 60 fps claim: B {fps_b:.1} / C {fps_c:.1} below {floor}");
        }
    }

    /// Half the cores, 1..=8 (12.1.2: was at most 4).
    #[test]
    fn cpu_capture_threads_scale_to_eight() {
        assert_eq!(cpu_capture_threads_for(1), 1);
        assert_eq!(cpu_capture_threads_for(8), 4);
        assert_eq!(cpu_capture_threads_for(16), 8);
        assert_eq!(cpu_capture_threads_for(32), 8);
    }

    /// The [lk/cadence] verdicts, from the field numbers (a 12.1.0 field log: cap 60, ~36 fps out; RC1 first-start: 19 fps).
    #[test]
    fn cadence_verdict_names_the_stage() {
        let m = |src, out, scale_ms, chain_ms: f64, busy| Cadence { chain_measured: true, cap: 60.0, src, out, scale_ms, chain_ms, chain_busy: busy };
        assert_eq!(cadence_verdict(&m(100.0, 59.0, 3.0, 4.0, 0.3)), "at the cap (within 10 %)");
        assert_eq!(cadence_verdict(&m(0.0, 0.0, 0.0, 0.0, 0.0)), "no frames");
        assert_eq!(cadence_verdict(&Cadence { chain_measured: false, ..m(36.0, 36.0, 0.0, 0.0, 0.0) }), "below the cap; this pipeline's stages are not measured");
        // RC1 P1: source 19, chain busy most of the time -> the pipeline, NOT the compositor
        assert_eq!(cadence_verdict(&m(19.5, 19.5, 2.0, 48.0, 0.94)), "the capture pipeline (upload/scale/download) is the ceiling");
        // 12.1.0 CPU path: 36 fps x 25 ms in the scaler
        assert_eq!(cadence_verdict(&m(36.0, 36.0, 25.0, 0.0, 0.0)), "the capture pipeline (upload/scale/download) is the ceiling");
        // chain mostly idle, few buffers: the source
        assert!(cadence_verdict(&m(36.0, 36.0, 5.0, 6.0, 0.22)).starts_with("few buffers at the source pad"));
        assert_eq!(cadence_verdict(&m(60.0, 30.0, 5.0, 6.0, 0.36)), "frames lost between the source and the appsink");
    }

    /// Pacing proof for the CPU path: a LIVE 5120x2160 BGRx source at 60 fps through the real launch line (cap 60,
    /// 3408x1440 box like the field test's 1440p share) with the cadence probes installed. Always: frames flow. With
    /// TINHAT_PERF_TESTS=1 (an idle machine with >= 8 cores, release build): >= 54 fps at the appsink.
    #[test]
    fn cpu_capture_keeps_60_from_a_5k_source() {
        let _ = gst::init();
        let src = "videotestsrc is-live=true pattern=solid-color ! video/x-raw,format=BGRx,width=5120,height=2160,framerate=60/1";
        let desc = cpu_capture_desc(src, "", 60, cpu_capture_threads());
        let Ok(pipeline) = gst::parse::launch(&desc).map(|p| p.downcast::<gst::Pipeline>().unwrap()) else {
            eprintln!("skipping: the CPU capture pipeline cannot run here");
            return;
        };
        install_first_frame_repin(&pipeline, 3408, 1440, None, Default::default(), "test");
        install_cadence_probes(&pipeline, 60, "test CPU");
        let sink = pipeline.by_name("sink").unwrap().downcast::<gstreamer_app::AppSink>().unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();
        let (mut n, mut warm) = (0u32, None);
        let t0 = std::time::Instant::now();
        while t0.elapsed().as_secs_f64() < 7.0 {
            if sink.try_pull_sample(gst::ClockTime::from_mseconds(200)).is_none() {
                continue;
            }
            // count from 1 s in: the first second is negotiation + the re-pin
            if t0.elapsed().as_secs_f64() >= 1.0 {
                warm.get_or_insert_with(std::time::Instant::now);
                n += 1;
            }
        }
        let fps = n as f64 / warm.map(|w| w.elapsed().as_secs_f64()).unwrap_or(1.0);
        let _ = pipeline.set_state(gst::State::Null);
        eprintln!("cpu capture 5120x2160 -> 3408x1440 ({} threads): {fps:.1} fps", cpu_capture_threads());
        assert!(n > 0, "no frames");
        if std::env::var_os("TINHAT_PERF_TESTS").is_some() {
            assert!(fps >= 54.0, "{fps:.1} fps");
        }
    }
}

/// 12.1.4 lab case (BUG-20261030 finding 1): a live 60 fps source through the REAL share feeder
/// (`wire_appsink_to_source`) into a real `NativeVideoSource`, with a sink on its track recording the timestamps the
/// SDK hands on. Mid-share it injects ONE wrapped PTS (frame 120: a running time of -9.3 s read as u64 ns, as in
/// the field log) and later a PTS reset to 0 (frame 300: a capture restart). Run twice through the identical path: with
/// 12.1.3's stamping rule (test-only switch) and with 12.1.4's. Proves for 12.1.4: every delivered frame reaches the
/// track, timestamps strictly increase, and keep the 60 fps spacing after both events; and shows what 12.1.3's rule
/// turned the same events into at the same point. What libwebrtc's encoder then drops is NOT modelled here: the field
/// log (encoder fed ~half) and a real share are that evidence. Opt-in, run alone: TINHAT_PACING_LAB=1, filter
/// `pacing_lab` (the legacy switch is process-wide).
#[cfg(test)]
mod pacing_lab {
    use super::*;
    use futures_util::StreamExt;
    use livekit::webrtc::video_stream::native::NativeVideoStream;
    use std::sync::{Arc, Mutex};

    /// (frames delivered to the appsink, timestamps the track sink received)
    fn run(legacy: bool) -> (usize, Vec<i64>) {
        LEGACY_RULE_FOR_LAB.store(legacy, std::sync::atomic::Ordering::SeqCst);
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        let _guard = rt.enter();
        let source = NativeVideoSource::new(VideoResolution { width: 320, height: 240 }, true);
        let track = LocalVideoTrack::create_video_track("pacing-lab", RtcVideoSource::Native(source.clone()));
        let received: Arc<Mutex<Vec<i64>>> = Default::default();
        // the sink is attached HERE, before anything plays (no startup frames missed)
        let mut stream = NativeVideoStream::new(track.rtc_track());
        {
            let rx = received.clone();
            rt.spawn(async move {
                while let Some(frame) = stream.next().await {
                    rx.lock().unwrap().push(frame.timestamp_us);
                }
            });
        }
        let pipeline = gst::parse::launch(
            "videotestsrc is-live=true pattern=ball ! video/x-raw,format=I420,width=320,height=240,framerate=60/1 ! \
             identity name=inj ! appsink name=sink emit-signals=false max-buffers=8 drop=false sync=false",
        )
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
        let n_in = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let reset_at: Arc<Mutex<Option<u64>>> = Default::default();
        {
            let (n_in, reset_at) = (n_in.clone(), reset_at.clone());
            pipeline.by_name("inj").unwrap().static_pad("src").unwrap().add_probe(gst::PadProbeType::BUFFER, move |_, info| {
                let n = n_in.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if let Some(gst::PadProbeData::Buffer(buf)) = info.data.as_mut() {
                    let pts = buf.pts().map(|t| t.nseconds()).unwrap_or(0);
                    let new = if n == 120 {
                        Some(u64::MAX - 9_300_000_000)
                    } else if n >= 300 {
                        let base = *reset_at.lock().unwrap().get_or_insert(pts);
                        Some(pts - base)
                    } else {
                        None
                    };
                    if let Some(v) = new {
                        buf.make_mut().set_pts(gst::ClockTime::from_nseconds(v));
                    }
                }
                gst::PadProbeReturn::Ok
            });
        }
        let delivered = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sink = pipeline.by_name("sink").unwrap().downcast::<gstreamer_app::AppSink>().unwrap();
        {
            let delivered = delivered.clone();
            sink.static_pad("sink").unwrap().add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                delivered.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                gst::PadProbeReturn::Ok
            });
        }
        wire_appsink_to_source(&sink, source.clone());
        pipeline.set_state(gst::State::Playing).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(9));
        let _ = pipeline.set_state(gst::State::Null);
        // drain the sink (the keepalive repeater ends with the pipeline's appsink)
        std::thread::sleep(std::time::Duration::from_millis(500));
        LEGACY_RULE_FOR_LAB.store(false, std::sync::atomic::Ordering::SeqCst);
        let got = received.lock().unwrap().clone();
        (delivered.load(std::sync::atomic::Ordering::SeqCst), got)
    }

    /// (median step µs, smallest step µs, steps under 1 ms) from frame 130 on (after both events)
    fn after_events(v: &[i64]) -> (i64, i64, usize, usize) {
        let tail = &v[130.min(v.len())..];
        let mut d: Vec<i64> = tail.windows(2).map(|w| w[1] - w[0]).collect();
        d.sort();
        let tiny = d.iter().filter(|&&x| x < 1_000).count();
        (d[d.len() / 2], d[0], tiny, d.len())
    }

    #[test]
    fn pts_jumps_mid_share_keep_every_frame_and_the_spacing() {
        if std::env::var_os("TINHAT_PACING_LAB").is_none() {
            eprintln!("skipping: TINHAT_PACING_LAB not set");
            return;
        }
        let _ = gst::init();
        let (d_old, old) = run(true);
        let (d_new, new) = run(false);
        let (med_old, min_old, tiny_old, n_old) = after_events(&old);
        let (med_new, min_new, tiny_new, n_new) = after_events(&new);
        let regress = |v: &[i64]| v.windows(2).filter(|w| w[1] <= w[0]).count();
        eprintln!("[pacing-lab] wrapped PTS injected at frame 120, PTS reset to 0 at frame 300; 9 s at 60 fps, both runs through the same feeder + SDK path");
        eprintln!(
            "[pacing-lab] 12.1.3 rule: delivered {d_old}, the track got {}; after the events: median step {:.3} ms, smallest {:.3} ms, {tiny_old}/{n_old} steps under 1 ms, {} non-increasing",
            old.len(), med_old as f64 / 1000.0, min_old as f64 / 1000.0, regress(&old)
        );
        eprintln!(
            "[pacing-lab] 12.1.4 rule: delivered {d_new}, the track got {}; after the events: median step {:.3} ms, smallest {:.3} ms, {tiny_new}/{n_new} steps under 1 ms, {} non-increasing",
            new.len(), med_new as f64 / 1000.0, min_new as f64 / 1000.0, regress(&new)
        );
        assert!(d_new > 450, "only {d_new} frames delivered");
        assert!(new.len() + 2 >= d_new, "the track got {} of {d_new} delivered frames", new.len());
        assert_eq!(regress(&new), 0, "timestamps handed on must strictly increase");
        assert!((15_000..=18_500).contains(&med_new), "median step {med_new} µs after the events");
        assert!(tiny_new <= 2, "{tiny_new} sub-millisecond steps after the events");
        assert!(med_old < 15_000 || tiny_old > n_old / 2, "the comparison run should show 12.1.3's collapse");
    }
}

#[cfg(test)]
mod send_gate_tests {
    //! 12.1.5 (assembly-041): the vendored livekit's send gate that keeps offers back while a new sender has
    //! no cryptor yet (peer_transport.rs `send_gate`, rtc_session.rs `create_sender`,
    //! local_participant.rs `publish_track`).
    use livekit::rtc_engine::take_offer_gate;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::RwLock as AsyncRwLock;

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
    }

    #[test]
    fn an_offer_waits_for_a_publish_still_attaching_and_is_skipped_on_expiry() {
        rt().block_on(async {
            let gate = Arc::new(AsyncRwLock::new(()));
            let held = gate.clone().read_owned().await; // a publish between add_transceiver and its attach
            assert!(take_offer_gate(&gate, Duration::from_millis(30)).await.is_none(), "no offer while a sender has no cryptor");
            drop(held); // attached
            assert!(take_offer_gate(&gate, Duration::from_millis(30)).await.is_some(), "the offer goes once attached");
        });
    }

    #[test]
    fn a_publish_waits_while_an_offer_is_being_created_then_goes() {
        rt().block_on(async {
            let gate = Arc::new(AsyncRwLock::new(()));
            let offer = take_offer_gate(&gate, Duration::from_millis(30)).await.expect("free gate");
            // a sender added now would be in neither this offer nor (without the gate) safe from the next one
            assert!(tokio::time::timeout(Duration::from_millis(30), gate.clone().read_owned()).await.is_err());
            drop(offer); // the offer is the local description now
            assert!(tokio::time::timeout(Duration::from_millis(30), gate.clone().read_owned()).await.is_ok());
        });
    }

    #[test]
    fn two_publishes_hold_the_gate_together_and_the_offer_waits_for_both() {
        rt().block_on(async {
            let gate = Arc::new(AsyncRwLock::new(()));
            let a = gate.clone().read_owned().await;
            let b = gate.clone().read_owned().await; // a full reconnect republishes concurrently
            drop(a);
            assert!(take_offer_gate(&gate, Duration::from_millis(30)).await.is_none());
            drop(b);
            assert!(take_offer_gate(&gate, Duration::from_millis(30)).await.is_some());
        });
    }

    fn region<'a>(src: &'a str, from: &str, to: &str) -> &'a str {
        let i = src.find(from).unwrap_or_else(|| panic!("missing {from:?}"));
        let j = i + src[i..].find(to).unwrap_or_else(|| panic!("missing {to:?} after {from:?}"));
        &src[i..j]
    }

    #[test]
    fn the_held_region_never_awaits_and_negotiation_comes_after_the_attach() {
        // the publish's hold must never wait on the offer path or the transport lock: no .await while held
        let lp = include_str!("../../../vendor/livekit/src/room/participant/local_participant.rs");
        let held = region(lp, "let (transceiver, send_gate) =", "send_gate.commit();");
        let after_create = &held[held.find(".await?;").expect("create_sender awaited") + ".await?;".len()..];
        assert!(!after_create.contains(".await"), "publish_track awaits while holding the send gate");
        assert!(after_create.contains("local_track_published(self.clone(), publication.clone())"), "the attach hook runs under the gate");
        assert!(!after_create.contains("publisher_negotiation_needed"), "negotiation requested before the attach");
        let tail = region(lp, "send_gate.commit();", "Ok(publication)");
        assert!(tail.contains("publisher_negotiation_needed()"), "the negotiation request (and retry) after the attach");

        let rs = include_str!("../../../vendor/livekit/src/rtc_engine/rtc_session.rs");
        let held = region(rs, "let mut gate = self.publisher_pc.hold_send_gate().await;", "Ok((transceiver, gate))");
        assert!(!held[held.find(";").unwrap()..].contains(".await"), "create_sender awaits while holding the send gate");
        assert!(held.contains("add_transceiver(track.rtc_track(), init)"), "the transceiver is added under the gate");
        assert!(held.contains("let _ = transceiver.stop();"), "a sender returned without a handle is stopped (fail closed)");

        let pt = include_str!("../../../vendor/livekit/src/rtc_engine/peer_transport.rs");
        let offer = region(pt, "pub async fn create_and_send_offer", "set_local_description(offer.clone())");
        let g = offer.find("take_offer_gate(&self.send_gate, SEND_GATE_WAIT)").expect("the offer takes the gate");
        let l = offer.find("self.inner.lock().await").expect("the transport lock");
        assert!(g < l, "the offer takes the send gate before the transport lock");

        // fail closed: the guard is armed with the new transceiver at once and stops it unless committed
        assert!(rs.contains("self.publisher_pc.peer_connection().add_transceiver(track.rtc_track(), init)?;\n        gate.arm(transceiver.clone());"),
            "the guard is armed right after add_transceiver");
        let guard_drop = region(pt, "impl Drop for SendGateGuard", "pub const SEND_GATE_HELD");
        assert!(guard_drop.contains("if !self.committed") && guard_drop.contains("t.stop()"), "an uncommitted guard stops its sender");
        // the answer path releases the transport lock before it renegotiates (the offer takes the gate, then that lock)
        let answer = region(pt, "pub async fn set_remote_description", "Ok(())\n    }");
        let d = answer.find("drop(inner);").expect("the transport lock released");
        let o = answer.find("self.create_and_send_offer(").expect("the renegotiation offer");
        assert!(d < o, "renegotiation offer made while holding the transport lock");
        // a held-back offer is retried on both negotiation paths
        let fast = region(rs, "async fn execute_negotiation_with_retry", "fn debounce_negotiation");
        assert!(fast.contains("is_send_gate_held(&err)") && fast.contains("NegotiationState::PendingRetry;"), "the fast path retries a held-back offer");
        assert!(fast.contains("*state = NegotiationState::Idle;\n                    self.negotiation_queue.task_running.store(false"), "the worker's exit is atomic with Idle");
        // every held-back offer (debounced, fast, the answer path's renegotiation) asks the session again
        assert!(offer.contains("self.on_offer_held.lock().as_ref()"), "a held-back offer calls the re-ask hook");
        let hook = region(rs, "is asked for again,", "// Start session tasks");
        assert!(hook.contains("inner.publisher_negotiation_needed()") && hook.contains("Arc::downgrade(&inner)"), "the hook asks the session's own negotiation path");
        // a debouncer that has fired refuses call() at once, so a request made while (or after) it offers starts a new one
        let db = include_str!("../../../vendor/livekit/src/utils/debouncer.rs");
        let fired = region(db, "_ = livekit_runtime::sleep(duration) => {", "break;");
        assert!(fired.find("rx.close();").expect("rx closed") < fired.find("future.await;").expect("the future"),
            "the debouncer must stop taking requests before its future runs");
        // the re-ask is coalesced and skipped for a closed session; a closed session's fast worker stops
        assert!(hook.contains("held_retry_scheduled.swap(true") && hook.contains("held_retry_scheduled.store(false"), "re-asks are coalesced");
        assert!(fast.contains("if self.closed.load(Ordering::Acquire) {"), "a closed session's worker stops");
        // the fast path goes only through the state-locked queue
        let need = region(rs, "fn publisher_negotiation_needed(self: &Arc<Self>)", "fn queue_negotiation");
        assert!(!need.contains("NegotiationState::PendingRetry"), "an unlocked PendingRetry write is back");
    }
}
