//! tinhat-publish — dev harness for the streamer core.
//!
//!   tinhat-publish local      videotestsrc -> nvh264enc -> fakesink  (no network)
//!   tinhat-publish publish     videotestsrc -> nvh264enc -> livekit room (reads env)
//!
//! Duration via LK_SECONDS (default 10). `publish` reads LIVEKIT_URL / LIVEKIT_API_KEY
//! / LIVEKIT_API_SECRET / LK_ROOM / LK_IDENTITY.

use anyhow::{Context, Result};
use gstreamer as gst;
use tinhat_streamer as streamer;

fn main() -> Result<()> {
    // RUST_LOG=info surfaces libwebrtc's own BWE decisions (probe clusters, REMB,
    // overuse, start-bitrate) through the log->tracing bridge — reading these is
    // how the bandwidth-ramp analysis stopped being guesswork.
    if std::env::var_os("RUST_LOG").is_some() {
        let _ = tracing_log::LogTracer::init();
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .try_init();
    }
    gst::init()?;
    streamer::force_hardware_h264();

    let mode = std::env::args().nth(1).unwrap_or_else(|| "local".into());
    let secs: u64 = std::env::var("LK_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);

    match mode.as_str() {
        "local" => {
            println!("[mode] local smoke: videotestsrc -> nvh264enc -> fakesink ({secs}s)");
            let p = streamer::build_local_smoke_pipeline()?;
            streamer::run_for(&p, secs)?;
            println!("[ok] local NVENC pipeline ran clean");
        }
        "publish" => {
            let cfg = streamer::PublishConfig::from_env()?;
            println!("[mode] publish -> room={} url={} ({secs}s)", cfg.room, cfg.ws_url);
            let p = streamer::build_publish_pipeline(&cfg)?;
            streamer::run_for(&p, secs)?;
            println!("[ok] publish pipeline ran clean");
        }
        "publish-token" => {
            // Real-app path: authenticate with a pre-minted lk-jwt-service JWT.
            let ws_url = std::env::var("LIVEKIT_URL").context("LIVEKIT_URL not set")?;
            let jwt = std::env::var("LK_TOKEN").context("LK_TOKEN not set")?;
            let room = std::env::var("LK_ROOM").unwrap_or_else(|_| "nvenc-token-test".into());
            println!("[mode] publish-token -> room={room} url={ws_url} ({secs}s)");
            let p = streamer::build_publish_pipeline_with_token(&ws_url, &jwt, &room)?;
            streamer::run_for(&p, secs)?;
            println!("[ok] token publish pipeline ran clean");
        }
        "conntest" => {
            // Headless run of the connection probe — the same code run_connection_test
            // calls — so the bandwidth-ramp fixes are testable from a GPU-less builder (the
            // ladder must fall to software VP9 and the report must SAY so).
            let ws_url = std::env::var("LIVEKIT_URL").context("LIVEKIT_URL not set")?;
            let jwt = std::env::var("LK_TOKEN").context("LK_TOKEN not set")?;
            println!(
                "[mode] conntest -> url={ws_url} ({secs}s, cap {} Mbps)",
                streamer::PROBE_CAP_BPS / 1_000_000
            );
            let rt = tokio::runtime::Runtime::new()?;
            let r = rt.block_on(streamer::connection_probe(&ws_url, &jwt, secs as u32))?;
            println!(
                "[ok] probe: peak={} kbps sustained={} kbps send={} kbps bwe={} kbps path={} encoder={} still_rising={}",
                r.peak_grant_kbps, r.tail_grant_kbps, r.send_kbps, r.bwe_kbps, r.path,
                r.encoder_label, r.still_rising
            );
        }
        other => {
            anyhow::bail!("unknown mode {other:?} — use 'local', 'publish', 'publish-token', or 'conntest'")
        }
    }
    Ok(())
}
