//! Memories — the headless call-recorder service (0.8.7).
//!
//! One instance, on the recorder host, systemd-run. Forever: sync → accept invites → find a room
//! whose `dev.tinhat.record` marker is on AND whose call has live members → join
//! it as a VISIBLE participant (the client shows 🔴 REC off our presence) and let the
//! streamer capture per-track files under `TINHAT_VOD_DIR/<room>/<stamp>/`. Stops when
//! the marker clears or the call empties. ONE recording at a time (the call stack is
//! single-call by design); other marked rooms wait and are picked up next loop.
//!
//! CONSENT: before and during a session the "Never record me" set is refreshed from
//! the membership markers (`rtc::record_optout_mxids`) into the streamer, which is
//! where the guarantee is enforced — an excluded identity never gets a capture chain.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
        .collect()
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn write_session_start(dir: &str, room_id: &str, room_name: &str, by: &str, start_ms: u128) {
    let v = serde_json::json!({
        "room": room_id, "room_name": room_name, "started_by": by,
        "start_ms": start_ms as u64, "end_ms": null,
    });
    let _ = std::fs::write(format!("{dir}/session.json"), v.to_string());
}

/// Read-modify-write: the end stamp must not clobber room_name & friends.
fn write_session_end(dir: &str, end_ms: u128) {
    let p = format!("{dir}/session.json");
    let mut v = std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    v["end_ms"] = serde_json::json!(end_ms as u64);
    let _ = std::fs::write(&p, v.to_string());
}

/// Sign-correct "s.mmm" seconds from a millisecond count. Rust's integer division
/// truncates toward zero, so the old `{}.{:03}` formatter dropped the sign for
/// -999..-1 ms (-166 -> "0.166"): format from the absolute value, prepend the sign.
fn fmt_secs(ms: i64) -> String {
    let sign = if ms < 0 { "-" } else { "" };
    let a = ms.unsigned_abs();
    format!("{sign}{}.{:03}", a / 1000, a % 1000)
}

/// First video stream start_time in seconds via ffprobe (None on any failure). A
/// combined file MUST start its video at ~0: the player maps mpv time-pos 0 to the
/// track's start_ms, so any leading offset here is a timeline error of that size.
fn probe_video_start_secs(path: &str) -> Option<f64> {
    let o = std::process::Command::new("ffprobe")
        .args([
            "-v", "error", "-select_streams", "v:0", "-show_entries", "stream=start_time",
            "-of", "csv=p=0", path,
        ])
        .output()
        .ok()?;
    if !o.status.success() {
        return None;
    }
    String::from_utf8_lossy(&o.stdout).trim().parse::<f64>().ok()
}

/// Number of packets in the first audio stream via ffprobe (None on any failure). The
/// readability test for the mixdown: a capture that opened but never received a frame is
/// a header-only file with 0 packets; a real capture has thousands. Deliberately NOT the
/// container duration — a streamable-Matroska capture has none until the seekability remux, and
/// judging by duration (0.8.21 → 0.8.31) rejected every capture of every session.
fn probe_audio_packets(path: &str) -> Option<u64> {
    let o = std::process::Command::new("ffprobe")
        .args([
            "-v", "error", "-select_streams", "a:0", "-count_packets",
            "-show_entries", "stream=nb_read_packets", "-of", "csv=p=0", path,
        ])
        .output()
        .ok()?;
    if !o.status.success() {
        return None;
    }
    String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().ok()
}

/// Seekability remux, in place: matroskamux streamable=true writes NO cues and no
/// duration — mpv cannot seek the captures and ffprobe reports no length. Stream-copy each
/// capture to a temp name and rename over the original. `all=false` (the startup sweep)
/// touches only captures that still lack a duration — the leftovers of the 0.8.21 bug.
fn remux_captures(dir: &str, all: bool) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name == "mixed.mka"
            || name.starts_with("combined-")
            || name.starts_with(".remux-")
            || !(name.ends_with(".mka") || name.ends_with(".mkv"))
        {
            continue;
        }
        let src = format!("{dir}/{name}");
        if !all && probe_duration_secs(&src).is_some() {
            continue; // already seekable
        }
        if std::fs::metadata(&src).map(|m| m.len() < 1024).unwrap_or(true) {
            continue; // header-only capture: nothing to remux, the mixdown skips it
        }
        let tmp = format!("{dir}/.remux-{name}");
        let ok = std::process::Command::new("ffmpeg")
            .args(["-y", "-i", &src, "-c", "copy", &tmp])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok && std::fs::rename(&tmp, &src).is_ok() {
            eprintln!("[vod] remuxed (seekable) {name}");
        } else {
            let _ = std::fs::remove_file(&tmp);
            eprintln!("[vod] remux FAILED for {name} (kept original)");
        }
    }
}

/// Container duration in seconds via ffprobe (None on any failure).
fn probe_duration_secs(path: &str) -> Option<f64> {
    let o = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", path])
        .output()
        .ok()?;
    if !o.status.success() {
        return None;
    }
    String::from_utf8_lossy(&o.stdout).trim().parse::<f64>().ok()
}

/// The session's end stamp, if session.json has one.
fn session_end_ms(dir: &str) -> Option<u64> {
    let v = std::fs::read_to_string(format!("{dir}/session.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())?;
    v.get("end_ms").and_then(|x| x.as_u64())
}

/// R3: stamp `end_ms` (track file mtime) into every tracks.jsonl line that lacks
/// it. Adds fields only -- old lines and old readers keep working. Must run BEFORE
/// the seekability remux, which rewrites the files (and their mtimes).
fn enrich_tracks_end_ms(dir: &str) {
    let p = format!("{dir}/tracks.jsonl");
    let Ok(text) = std::fs::read_to_string(&p) else { return };
    let mut out = String::new();
    let mut changed = false;
    for line in text.lines() {
        let Ok(mut v) = serde_json::from_str::<serde_json::Value>(line) else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        if v.get("end_ms").is_none() {
            if let Some(ms) = v
                .get("file")
                .and_then(|x| x.as_str())
                .and_then(|f| std::fs::metadata(format!("{dir}/{f}")).ok())
                .and_then(|md| md.modified().ok())
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
            {
                v["end_ms"] = serde_json::json!(ms);
                changed = true;
            }
        }
        out.push_str(&v.to_string());
        out.push('\n');
    }
    if changed {
        if std::fs::write(&p, out).is_ok() {
            eprintln!("[vod] tracks.jsonl enriched with end_ms -> {dir}");
        } else {
            eprintln!("[vod] tracks.jsonl enrich WRITE FAILED -> {dir}");
        }
    }
}

/// A CLOSED session whose post-close pipeline never finished: audio tracks exist but
/// there is no mixed.mka, or a video track has no combined-<file>. This is exactly
/// the API's `ready == false` (the player's "Processing" card), so anything true here
/// is stuck until someone reruns the pipeline. Video-only sessions (no audio) are
/// left alone -- there is nothing to mix.
fn session_unfinished(dir: &str) -> bool {
    let Ok(tracks) = std::fs::read_to_string(format!("{dir}/tracks.jsonl")) else { return false };
    let mut has_audio = false;
    let mut missing_combined = false;
    for line in tracks.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let kind = v.get("kind").and_then(|k| k.as_str()).unwrap_or("");
        let Some(f) = v.get("file").and_then(|x| x.as_str()) else { continue };
        if kind == "audio" {
            has_audio = true;
        } else if !std::path::Path::new(&format!("{dir}/combined-{f}")).exists() {
            missing_combined = true;
        }
    }
    has_audio && (!std::path::Path::new(&format!("{dir}/mixed.mka")).exists() || missing_combined)
}

/// R4: one-shot startup repair. (a) A service that died mid-session leaves
/// session.json with end_ms=null and no mixdown/combined outputs: stamp end_ms from
/// the newest track file's mtime and run the normal post-close pipeline. (b) A closed
/// session whose pipeline died half-way (see session_unfinished) gets the mixdown +
/// combined steps again, without the remux (done at close). Guard: only sessions
/// quiet for >10 minutes are touched, so a recorder restarting mid-call never grabs
/// the live session (the room marker re-triggers recording on its own).
fn repair_crashed_sessions(base: &str) {
    let now = std::time::SystemTime::now();
    let Ok(rooms) = std::fs::read_dir(base) else { return };
    for room in rooms.flatten() {
        if !room.path().is_dir() {
            continue;
        }
        let Ok(sessions) = std::fs::read_dir(room.path()) else { continue };
        for sess in sessions.flatten() {
            let sp = sess.path();
            if !sp.is_dir() {
                continue;
            }
            let dir = sp.to_string_lossy().to_string();
            let open_ended = std::fs::read_to_string(format!("{dir}/session.json"))
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| v.get("end_ms").map(|e| e.is_null()))
                .unwrap_or(false);
            let closed_unfinished = !open_ended && session_unfinished(&dir);
            if !open_ended && !closed_unfinished {
                continue;
            }
            // Newest track-file mtime = last moment the recorder wrote anything.
            let mut newest: Option<std::time::SystemTime> = None;
            if let Ok(files) = std::fs::read_dir(&sp) {
                for f in files.flatten() {
                    let name = f.file_name().to_string_lossy().to_string();
                    if name == "mixed.mka"
                        || name.starts_with("combined-")
                        || name.starts_with(".remux-")
                        || !(name.ends_with(".mka") || name.ends_with(".mkv"))
                    {
                        continue;
                    }
                    if let Ok(m) = f.metadata().and_then(|md| md.modified()) {
                        newest = Some(newest.map_or(m, |n| n.max(m)));
                    }
                }
            }
            let Some(newest) = newest else {
                eprintln!("[vod] REPAIR skip (no track files): {dir}");
                continue;
            };
            let quiet = now.duration_since(newest).map(|d| d.as_secs()).unwrap_or(0);
            if quiet < 600 {
                eprintln!("[vod] REPAIR skip (only {quiet}s quiet, <10 min): {dir}");
                continue;
            }
            let end_ms = newest
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            if open_ended {
                eprintln!(
                    "[vod] REPAIR crashed session {dir}: stamping end_ms={end_ms} + post-close pipeline"
                );
                write_session_end(&dir, end_ms);
                post_close_pipeline(&dir, true);
            } else {
                eprintln!("[vod] REPAIR unfinished closed session {dir}: mixdown + combined");
                post_close_pipeline(&dir, false);
            }
            eprintln!("[vod] REPAIR done: {dir}");
        }
    }
}

/// One MIXED chat-audio file per session so the player needs a single audio surface;
/// per-voice offsets come from tracks.jsonl. Blocking ffmpeg on a plain thread — a
/// session has just ended, nothing is waiting on us.
fn spawn_mixdown(dir: String) {
    std::thread::spawn(move || post_close_pipeline(&dir, true));
}

/// Everything that runs after a session ends: R3 track-metadata enrichment, the
/// audio mixdown, the seekability remux, the combined per-POV muxes.
/// Blocking -- called on a plain thread at session close (spawn_mixdown) and
/// synchronously from the R4 startup crash repair.
fn post_close_pipeline(dir: &str, remux: bool) {
    // R3 first: the remux below rewrites the track files (and their mtimes).
    enrich_tracks_end_ms(dir);
    // Then the seekability remux — BEFORE the mixdown, so the mix reads seekable inputs with
    // a real duration (0.8.21–0.8.31 ran it after, behind the early return below: it never
    // ran at all once the mixdown had rejected every un-remuxed capture).
    remux_captures(dir, remux);
    {
        let tracks = std::fs::read_to_string(format!("{dir}/tracks.jsonl")).unwrap_or_default();
        let mut inputs: Vec<(String, u64)> = Vec::new();
        let mut min_start = u64::MAX;
        for line in tracks.lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
            if v.get("kind").and_then(|k| k.as_str()) != Some("audio") {
                continue;
            }
            let (Some(f), Some(s)) = (
                v.get("file").and_then(|x| x.as_str()),
                v.get("start_ms").and_then(|x| x.as_u64()),
            ) else {
                continue;
            };
            // The POV origin (earliest audio start) counts EVERY audio track, skipped
            // ones included -- the combined step below uses the same origin, so
            // skipping an input never shifts the timeline.
            min_start = min_start.min(s);
            // A capture that opened but never received a frame is a 173-byte
            // header-only file; ffmpeg fails the WHOLE amix on it (exit 187 =
            // AVERROR_EOF) and the session then shows "Processing" forever. Feed
            // the mix only what ffprobe can read.
            match probe_audio_packets(&format!("{dir}/{f}")) {
                Some(n) if n > 0 => inputs.push((f.to_string(), s)),
                _ => eprintln!("[vod] mixdown: skipping unreadable/empty audio capture {f}"),
            }
        }
        if inputs.is_empty() {
            eprintln!("[vod] mixdown: no readable audio captures in {dir}");
            return;
        }
        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.arg("-y");
        for (f, _) in &inputs {
            cmd.arg("-i").arg(format!("{dir}/{f}"));
        }
        let mut fc = String::new();
        for (i, (_, s)) in inputs.iter().enumerate() {
            let d = s - min_start;
            fc.push_str(&format!("[{i}:a]adelay={d}|{d}[a{i}];"));
        }
        for i in 0..inputs.len() {
            fc.push_str(&format!("[a{i}]"));
        }
        fc.push_str(&format!("amix=inputs={}:normalize=0[out]", inputs.len()));
        cmd.arg("-filter_complex").arg(&fc).arg("-map").arg("[out]");
        cmd.arg("-c:a").arg("libopus").arg("-b:a").arg("128k");
        // R1: bound the mix to the session span when the end stamp is known. NOT
        // -shortest -- that would truncate the mix to the shortest audio track.
        if let Some(end) = session_end_ms(dir) {
            let span_ms = end as i64 - min_start as i64;
            if span_ms > 0 {
                cmd.arg("-t").arg(fmt_secs(span_ms));
            }
        }
        cmd.arg(format!("{dir}/mixed.mka"));
        match cmd.output() {
            Ok(o) if o.status.success() => eprintln!("[vod] mixdown OK -> {dir}/mixed.mka"),
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stderr);
                let mut last: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
                let keep = last.len().saturating_sub(3);
                let tail = last.split_off(keep).join(" | ");
                eprintln!("[vod] mixdown FAILED ({}): {tail}", o.status);
            }
            Err(e) => eprintln!("[vod] mixdown spawn failed: {e}"),
        }
        // COMBINED per-POV files: video + the aligned mixed audio in ONE mkv
        // (stream copy). The player prefers these — a single ordinary file gives
        // mpv-native lipsync AND paints (external-audio attach blacks mpvqt's FBO).
        if std::path::Path::new(&format!("{dir}/mixed.mka")).exists() {
            let tracks2 = std::fs::read_to_string(format!("{dir}/tracks.jsonl")).unwrap_or_default();
            let mut min_a: Option<u64> = None;
            let mut vids: Vec<(String, u64)> = Vec::new();
            for line in tracks2.lines() {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
                let kind = v.get("kind").and_then(|k| k.as_str()).unwrap_or("");
                let (Some(f), Some(s)) = (
                    v.get("file").and_then(|x| x.as_str()),
                    v.get("start_ms").and_then(|x| x.as_u64()),
                ) else {
                    continue;
                };
                if kind == "audio" {
                    min_a = Some(min_a.map_or(s, |m| m.min(s)));
                } else {
                    vids.push((f.to_string(), s));
                }
            }
            if let Some(a0) = min_a {
                for (f, vs) in vids {
                    // R1: a video that started LATER than the first audio needs the
                    // mixed audio ADVANCED by skip = vs - a0. The old `-itsoffset
                    // {vs-a0}` on the audio DELAYED it (2x lag for late joiners) and
                    // the mux ran to the mix's length. And input-side `-ss` on the
                    // audio is NOT copy-safe either: it seeks to the Matroska
                    // cluster before the target (5s) and ffmpeg shifts the video to
                    // compensate — leading audio + video start_time off 0 = a
                    // timeline error of up to 5s (measured with a synthetic click).
                    // Sample-accurate stream-copy form (+-1ms measured): shift the
                    // VIDEO later by skip, then cut everything before skip at the
                    // OUTPUT. Every Opus packet is a keyframe, the video's first
                    // packet lands exactly on the cut, so nothing is lost.
                    let skip_ms = vs as i64 - a0 as i64;
                    let vdur = probe_duration_secs(&format!("{dir}/{f}"));
                    let out = format!("{dir}/combined-{f}");
                    let mut cmd = std::process::Command::new("ffmpeg");
                    cmd.arg("-y");
                    if skip_ms >= 0 {
                        cmd.arg("-itsoffset").arg(fmt_secs(skip_ms));
                        cmd.arg("-i").arg(format!("{dir}/{f}"));
                        cmd.arg("-i").arg(format!("{dir}/mixed.mka"));
                        cmd.arg("-ss").arg(fmt_secs(skip_ms));
                    } else {
                        // theoretical: video began BEFORE the first audio -> delay
                        // the audio (a late-starting audio stream is fine for mpv)
                        cmd.arg("-i").arg(format!("{dir}/{f}"));
                        cmd.arg("-itsoffset").arg(fmt_secs(-skip_ms));
                        cmd.arg("-i").arg(format!("{dir}/mixed.mka"));
                    }
                    cmd.args(["-map", "0:v", "-map", "1:a", "-c", "copy"]);
                    // Bound the mux to the video's length (the mixed audio usually
                    // runs longer); -shortest only if the probe failed.
                    match vdur {
                        Some(d) => {
                            cmd.arg("-t").arg(format!("{d:.3}"));
                        }
                        None => {
                            cmd.arg("-shortest");
                        }
                    }
                    cmd.arg(&out);
                    let ok = cmd.output().map(|o| o.status.success()).unwrap_or(false);
                    if ok {
                        eprintln!(
                            "[vod] combined POV file -> combined-{f} (audio skip {}s)",
                            fmt_secs(skip_ms)
                        );
                        if let (Some(cd), Some(vd)) = (probe_duration_secs(&out), vdur) {
                            if (cd - vd).abs() > 2.0 {
                                eprintln!(
                                    "[vod] COMBINED DURATION MISMATCH {out} combined={cd:.3} video={vd:.3}"
                                );
                            }
                        }
                        if let Some(vs0) = probe_video_start_secs(&out) {
                            if vs0.abs() > 0.5 {
                                eprintln!(
                                    "[vod] COMBINED VIDEO START MISMATCH {out} video start_time={vs0:.3} (expected ~0)"
                                );
                            }
                        }
                    } else {
                        let _ = std::fs::remove_file(&out);
                        eprintln!("[vod] combined FAILED for {f}");
                    }
                }
            }
        }
    }
}

/// Live call members that are NOT us (the bot). Empty = nothing to record.
async fn others_in_call(room: &matrix_sdk::Room) -> usize {
    let own = room.own_user_id().to_string();
    crate::rtc::call_member_devices(room).await.iter().filter(|(u, _)| *u != own).count()
}

pub async fn run() {
    let base = std::env::var("TINHAT_VOD_DIR").unwrap_or_else(|_| "/vods".to_string());
    if std::fs::create_dir_all(&base).is_err() {
        eprintln!("[vod] FATAL: cannot create TINHAT_VOD_DIR {base}");
        return;
    }
    // R4: one-shot crash-repair sweep (sessions a crash left with end_ms=null).
    // Background thread -- repairs run blocking ffmpeg and must not delay service
    // start; sessions are processed one at a time inside.
    {
        let sweep_base = base.clone();
        std::thread::spawn(move || repair_crashed_sessions(&sweep_base));
    }
    // Session: restore the recorder's saved session; first boot falls back to
    // TINHAT_USER/TINHAT_PASS (systemd env file) via the same headless login SAS uses.
    let client = match crate::bridge::restore_client().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[vod] restore failed ({e}); trying headless login");
            let (Ok(user), Ok(pass)) = (std::env::var("TINHAT_USER"), std::env::var("TINHAT_PASS"))
            else {
                eprintln!("[vod] FATAL: no session and no TINHAT_USER/TINHAT_PASS");
                return;
            };
            let server = match std::env::var("TINHAT_HOMESERVER").ok().filter(|s| !s.trim().is_empty()) {
                Some(s) => s,
                None => match crate::bridge::env_homeserver() {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("[vod] FATAL: no session and no server: {e} (TINHAT_HOMESERVER works too)");
                        return;
                    }
                },
            };
            match crate::bridge::headless_login(&server, &user, &pass).await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[vod] FATAL: headless login failed: {e}");
                    return;
                }
            }
        }
    };
    crate::session::set_client(client.clone());
    eprintln!("[vod] service up as {:?}", client.user_id().map(|u| u.to_string()));

    // (room_id, dir, stop flag, join task, started_by, start_ms)
    let mut active: Option<(String, String, Arc<AtomicBool>, tokio::task::JoinHandle<()>, String, u128)> =
        None;
    let mut tick: u64 = 0;
    // Rooms we just left → don't re-match until the cooldown passes (ghost-membership yo-yo guard).
    let mut cooldown: std::collections::HashMap<String, tokio::time::Instant> =
        std::collections::HashMap::new();

    loop {
        let _ = client
            .sync_once(
                crate::bridge::sync_settings()
                    .timeout(std::time::Duration::from_secs(10)),
            )
            .await;

        // Accept invites — being invitable is how a room opts INTO recording at all.
        for room in client.invited_rooms() {
            eprintln!("[vod] joining invited room {}", room.room_id());
            if let Err(e) = room.join().await {
                eprintln!("[vod] invite join failed: {e}");
            }
        }

        if let Some((room_id, dir, stop, task, by, start)) = active.take() {
            // Should this session end? Marker off, call emptied, or join task died.
            let mut done = task.is_finished();
            if !done {
                if let Ok(id) = matrix_sdk::ruma::RoomId::parse(&room_id) {
                    if let Some(room) = client.get_room(&id) {
                        let marker_on = crate::appstate::record_enabled(&room).await.is_some();
                        let peers = others_in_call(&room).await;
                        // Tick diagnostics (rate-limited): the first live gate showed a
                        // session that never stopped — these are the stop inputs.
                        tick += 1;
                        if tick % 4 == 0 {
                            eprintln!(
                                "[vod] tick marker_on={marker_on} peers={peers} pending={} in {room_id}",
                                tinhat_streamer::call::record_pending_len()
                            );
                        }
                        if !marker_on || peers == 0 {
                            eprintln!(
                                "[vod] stopping (marker_on={marker_on} peers={peers}) in {room_id}"
                            );
                            done = true;
                        } else {
                            // Refresh consent FIRST, then build chains for queued tracks —
                            // this ordering (not subscribe-time building) is the opt-out
                            // guarantee for late joiners.
                            tinhat_streamer::call::set_record_optouts(
                                crate::rtc::record_optout_mxids(&room).await,
                            );
                            tinhat_streamer::call::drain_record_pending();
                        }
                    } else {
                        done = true;
                    }
                }
            }
            if done {
                stop.store(true, Ordering::Relaxed);
                let _ = tokio::time::timeout(std::time::Duration::from_secs(15), task).await;
                tinhat_streamer::call::set_record_dir(None);
                tinhat_streamer::call::finalize_recordings();
                write_session_end(&dir, now_ms());
                let _ = (&by, &start); // kept in the tuple for the else-branch restore
                spawn_mixdown(dir.clone());
                eprintln!("[vod] session in {room_id} closed -> {dir}");
                cooldown.insert(room_id, tokio::time::Instant::now());
            } else {
                active = Some((room_id, dir, stop, task, by, start));
            }
        }

        if active.is_none() {
            // Find the next room that wants recording and has a live call.
            for room in client.joined_rooms() {
                let Some(marker) = crate::appstate::record_enabled(&room).await else { continue };
                if others_in_call(&room).await == 0 {
                    continue;
                }
                let room_id = room.room_id().to_string();
                if let Some(t) = cooldown.get(&room_id) {
                    if t.elapsed() < std::time::Duration::from_secs(180) {
                        continue; // just left this call — ghost memberships must not yo-yo us back
                    }
                }
                let by = marker.get("by").and_then(|b| b.as_str()).unwrap_or("?").to_string();
                let start = now_ms();
                let dir = format!("{base}/{}/{start}", sanitize(&room_id));
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    eprintln!("[vod] cannot create {dir}: {e}");
                    continue;
                }
                // Consent set FIRST, then the capture dir, then join — order matters:
                // a chain must never be built before the opt-outs are in place.
                tinhat_streamer::call::set_record_optouts(
                    crate::rtc::record_optout_mxids(&room).await,
                );
                tinhat_streamer::call::set_record_dir(Some(dir.clone()));
                let room_name = room.display_name().await.map(|d| d.to_string()).unwrap_or_default();
                write_session_start(&dir, &room_id, &room_name, &by, start);
                let stop = Arc::new(AtomicBool::new(false));
                let stop2 = stop.clone();
                let rid = room_id.clone();
                let task = crate::bridge::runtime()
                    .spawn(async move { crate::call_ctl::vod_join_until(&rid, stop2).await });
                eprintln!("[vod] RECORDING {room_id} (started_by={by}) -> {dir}");
                active = Some((room_id, dir, stop, task, by, start));
                break; // one at a time
            }
        }

        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}
