//! codecprobe — which codec and encoder does a screen-share publish really get?
//!
//! Publishes a synthetic 30 fps screen-share into a LiveKit room once per step of
//! `PROBE_SEQ` (comma list of `codec:backend`, e.g. `vp9:software,h264:nvenc`), the same
//! publish options the app's share uses, with a subscriber in the room so the SFU forwards
//! it. Per step it prints the sender capabilities' matching codecs, then (after ~8 s) the
//! outbound stream's encoder implementation, negotiated mime type, frame size and fps.
//! Steps run in ONE room session with an unpublish in between, like a quality change.
//!
//! Env: LIVEKIT_URL, PUB_TOKEN, SUB_TOKEN; PROBE_SEQ (default `vp9:software,h264:nvenc`);
//! PROBE_W/PROBE_H (default 1920x800); PROBE_FPS feed rate (default 30); PROBE_KBPS (default 8000); PROBE_SECS per step (default 8).
use anyhow::{Context, Result};
use livekit::options::{TrackPublishOptions, VideoCodec, VideoEncoderBackend, VideoEncoding};
use livekit::prelude::*;
use livekit::track::{LocalTrack, LocalVideoTrack, TrackSource};
use livekit::webrtc::prelude::*;
use livekit::webrtc::stats::RtcStats;
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn parse_step(s: &str) -> Option<(VideoCodec, VideoEncoderBackend)> {
    let (c, b) = s.split_once(':').unwrap_or((s, "auto"));
    let codec = match c {
        "vp8" => VideoCodec::VP8,
        "vp9" => VideoCodec::VP9,
        "h264" => VideoCodec::H264,
        "av1" => VideoCodec::AV1,
        _ => return None,
    };
    let backend = match b {
        "software" => VideoEncoderBackend::Software,
        "nvenc" => VideoEncoderBackend::Nvenc,
        "vaapi" => VideoEncoderBackend::Vaapi,
        "hardware" => VideoEncoderBackend::Hardware,
        _ => VideoEncoderBackend::Auto,
    };
    Some((codec, backend))
}

fn spawn_feeder(source: NativeVideoSource, w: u32, h: u32, fps: u32, stop: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let (wu, hu) = (w as usize, h as usize);
        // A busy, text-like background drawn once; each frame copies it and moves a bar across, so every frame
        // differs (an encoder skips static frames) while the feed itself stays cheap at any size.
        let base: Vec<u8> = (0..wu * hu).map(|i| (((i % wu) / 3 + (i / wu) * 7) % 220 + 16) as u8).collect();
        let period = std::time::Duration::from_secs_f64(1.0 / fps.max(1) as f64);
        let start = std::time::Instant::now();
        let mut n: u32 = 0;
        while !stop.load(Ordering::Relaxed) {
            let mut buf = I420Buffer::new(w, h);
            {
                let (y, _, _) = buf.data_mut();
                y[..wu * hu].copy_from_slice(&base);
                let x0 = ((n * 24) % w) as usize;
                for row in 0..hu {
                    let line = &mut y[row * wu..(row + 1) * wu];
                    let end = (x0 + 64).min(wu);
                    line[x0..end].fill(235);
                }
            }
            let frame = VideoFrame {
                rotation: VideoRotation::VideoRotation0,
                timestamp_us: start.elapsed().as_micros() as i64,
                buffer: buf,
                frame_metadata: None,
            };
            source.capture_frame(&frame);
            n += 1;
            let next = period * n;
            let now = start.elapsed();
            if next > now {
                std::thread::sleep(next - now);
            }
        }
    });
}

fn main() -> Result<()> {
    // RUST_LOG=livekit=debug prints the publisher offers and the SFU's answers (which codec each m-line carries).
    if std::env::var_os("RUST_LOG").is_some() {
        let _ = tracing_log::LogTracer::init();
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_writer(std::io::stderr)
            .try_init();
    }
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(run())
}

async fn run() -> Result<()> {
    let url = std::env::var("LIVEKIT_URL").context("LIVEKIT_URL")?;
    let pub_tok = std::env::var("PUB_TOKEN").context("PUB_TOKEN")?;
    let sub_tok = std::env::var("SUB_TOKEN").context("SUB_TOKEN")?;
    let seq = std::env::var("PROBE_SEQ").unwrap_or_else(|_| "vp9:software,h264:nvenc".into());
    let w: u32 = std::env::var("PROBE_W").ok().and_then(|v| v.parse().ok()).unwrap_or(1920);
    let h: u32 = std::env::var("PROBE_H").ok().and_then(|v| v.parse().ok()).unwrap_or(800);
    let secs: u64 = std::env::var("PROBE_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(8);
    let kbps: u64 = std::env::var("PROBE_KBPS").ok().and_then(|v| v.parse().ok()).unwrap_or(8000);
    let fps: u32 = std::env::var("PROBE_FPS").ok().and_then(|v| v.parse().ok()).unwrap_or(30);

    println!("[probe] encoder backends available: {:?}", VideoEncoderBackend::list_available().into_iter().collect::<Vec<_>>());
    let caps = PeerConnectionFactory::default().get_rtp_sender_capabilities(MediaType::Video);
    for c in &caps.codecs {
        if !c.mime_type.to_lowercase().ends_with("/rtx") {
            println!("[probe] sender cap: {} {}", c.mime_type, c.sdp_fmtp_line.clone().unwrap_or_default());
        }
    }

    let (sub_room, _sub_ev) = Room::connect(&url, &sub_tok, RoomOptions::default()).await.context("sub connect")?;
    let (room, _ev) = Room::connect(&url, &pub_tok, RoomOptions::default()).await.context("pub connect")?;
    println!("[probe] connected pub={} sub={}", room.local_participant().identity().0, sub_room.local_participant().identity().0);

    for step in seq.split(',') {
        let Some((codec, backend)) = parse_step(step.trim()) else {
            println!("[probe] bad step {step:?}");
            continue;
        };
        let source = NativeVideoSource::new(VideoResolution { width: w, height: h }, true);
        let track = LocalVideoTrack::create_video_track("screen", RtcVideoSource::Native(source.clone()));
        let stats_track = track.clone();
        let stop = Arc::new(AtomicBool::new(false));
        spawn_feeder(source, w, h, fps, stop.clone());
        let publication = room
            .local_participant()
            .publish_track(
                LocalTrack::Video(track),
                TrackPublishOptions {
                    video_codec: codec,
                    video_encoder: backend,
                    source: TrackSource::Screenshare,
                    video_encoding: Some(VideoEncoding { max_bitrate: kbps * 1000, max_framerate: fps as f64 }),
                    simulcast: false,
                    ..Default::default()
                },
            )
            .await
            .context("publish")?;
        tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
        let stats = stats_track.get_stats().await.unwrap_or_default();
        let mut codecs = std::collections::HashMap::new();
        for s in &stats {
            if let RtcStats::Codec(c) = s {
                codecs.insert(c.rtc.id.clone(), (c.codec.mime_type.clone(), c.codec.sdp_fmtp_line.clone()));
            }
        }
        let mut any = false;
        for s in &stats {
            if let RtcStats::OutboundRtp(o) = s {
                any = true;
                let (mime, fmtp) = codecs.get(&o.stream.codec_id).cloned().unwrap_or_default();
                println!(
                    "[probe] step {step}: RESULT enc='{}' mime={} fmtp='{}' {}x{}@{:.0} target={}kbps qlim={:?}",
                    o.outbound.encoder_implementation, mime, fmtp, o.outbound.frame_width, o.outbound.frame_height,
                    o.outbound.frames_per_second, (o.outbound.target_bitrate / 1000.0) as u64,
                    o.outbound.quality_limitation_reason,
                );
            }
        }
        if !any {
            println!("[probe] step {step}: RESULT no outbound-rtp stats");
        }
        stop.store(true, Ordering::Relaxed);
        let _ = room.local_participant().unpublish_track(&publication.sid()).await;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    room.close().await.ok();
    sub_room.close().await.ok();
    Ok(())
}
