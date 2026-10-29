//! Per-user join sounds (QOL 1.5.2) — the short sound that plays for everyone already
//! in a call when you walk in, like TeamSpeak/Discord.
//!
//! DESIGN: the person JOINING publishes their own sound, over the same LiveKit second
//! audio track the soundboard already uses (`play_url_in_call`). That choice does the
//! heavy lifting for free — the sound arrives as a normal soundboard-category stream,
//! so the per-person volume and mute controls we already ship apply to it. A join sound
//! nobody can turn down is a griefing tool, so this is not a detail.
//!
//! GUARDS, all of them because this is a noise source pointed at other people:
//!   • hard stop after [`MAX_MS`] — a "join sound" that is a three-minute song isn't one;
//!   • a per-process [`COOLDOWN`] so leave/rejoin spam can't machine-gun the room;
//!   • an opt-out per room (`dev.tinhat.joinsounds` state, `enabled: false`),
//!     checked by the joiner before publishing, so a room can turn the whole thing off.

use std::time::{Duration, Instant};

/// The published sound is cut off after this long, however long the file is.
const MAX_MS: u64 = 3_000;
/// Minimum gap between two of OUR join sounds, however many times we rejoin.
const COOLDOWN: Duration = Duration::from_secs(60);
/// Extended-profile key holding the chosen sound url (see settings.rs for the others).
pub const JOINSOUND_KEY: &str = "dev.tinhat.joinsound";
/// Room state event that can switch join sounds off for everyone in that room.
pub const ROOM_TOGGLE_TYPE: &str = "dev.tinhat.joinsounds";

fn last_played() -> &'static std::sync::Mutex<Option<Instant>> {
    static L: std::sync::OnceLock<std::sync::Mutex<Option<Instant>>> = std::sync::OnceLock::new();
    L.get_or_init(|| std::sync::Mutex::new(None))
}

/// Is our join sound allowed to play right now? Pure decision, no I/O, so it can be
/// gate-tested without a call. `now` is injected for the same reason.
///
/// Returns the reason when it declines, because "my join sound didn't play" is exactly
/// the kind of thing that otherwise gets reported as a mystery.
pub fn may_play(url: &str, room_enabled: bool, last: Option<Instant>, now: Instant) -> Result<(), &'static str> {
    if url.trim().is_empty() {
        return Err("no join sound set");
    }
    if !room_enabled {
        return Err("join sounds are off in this room");
    }
    if let Some(t) = last {
        if now.duration_since(t) < COOLDOWN {
            return Err("cooldown: rejoined too soon");
        }
    }
    Ok(())
}

/// Read the room's opt-out. Absent event (the normal case) means ENABLED — a feature
/// that requires every room to opt in is a feature nobody discovers.
async fn room_enabled(room_id: &str) -> bool {
    let Some(client) = crate::session::client() else { return true };
    let Ok(rid) = matrix_sdk::ruma::RoomId::parse(room_id) else { return true };
    let Some(room) = client.get_room(&rid) else { return true };
    match crate::appstate::get_state(&room, ROOM_TOGGLE_TYPE, "").await {
        Some(v) => v.get("enabled").and_then(|e| e.as_bool()).unwrap_or(true),
        None => true,
    }
}

/// Our configured join sound url ("" when unset).
pub async fn my_join_sound() -> String {
    let Some(client) = crate::session::client() else { return String::new() };
    let Some(uid) = client.user_id() else { return String::new() };
    crate::settings::get_profile_field(&client, uid, JOINSOUND_KEY)
        .await
        .unwrap_or_default()
}

/// Publish our join sound into the call we just joined, then cut it off at [`MAX_MS`].
pub async fn play_on_join(room_id: &str) {
    // 10.5.1: an uploaded join sound stored under an old API name plays from the current one
    let url = crate::proxy::rebase_url(&my_join_sound().await);
    let enabled = room_enabled(room_id).await;
    let last = *last_played().lock().unwrap();
    if let Err(why) = may_play(&url, enabled, last, Instant::now()) {
        eprintln!("[joinsound] skipped: {why}");
        return;
    }
    // sec-night 2026-10-27 (review low): the stored URL goes through the soundboard's one rule (M9, member-content.json
    // `soundRoute`) like a soundboard press, never straight to GStreamer (any URI scheme it knows: file://, smb://, a
    // stranger's host). The Settings picker only offers soundboard sounds, which all pass; Tin Hat Web does the same
    // (shared/call.ts engineSoundUrl).
    let play = crate::soundboard::playable_clip_url(&url).await;
    if play.is_empty() {
        eprintln!("[joinsound] skipped: not a sound this app plays (an upload on this server, myinstants, or another Tin Hat server's upload)");
        return;
    }
    // resolving may have fetched a signed link: still in THIS room's call? (a soundboard press checks the same; a switch
    // to another room's call meanwhile must not get this room's sound, which that room may have turned off: assembly-038)
    if !crate::call_ctl::in_active_call().await || !crate::call_ctl::in_call_room(room_id) {
        eprintln!("[joinsound] skipped: left the call (or moved to another room's call) while the sound's link was fetched");
        return;
    }
    *last_played().lock().unwrap() = Some(Instant::now());
    eprintln!("[joinsound] publishing join sound into {room_id}");
    if let Err(e) = tinhat_streamer::play_url_in_call(&play).await {
        eprintln!("[joinsound] publish failed: {e}");
        return;
    }
    // Cut it off ourselves rather than trusting the file's length: this is the only
    // thing standing between "a short sound" and someone publishing a whole track over
    // everyone else's conversation.
    tokio::time::sleep(Duration::from_millis(MAX_MS)).await;
    tinhat_streamer::stop_url_in_call();
    eprintln!("[joinsound] done (capped at {MAX_MS}ms)");
}

/// Headless gate for the decision table (no call, no server):
///   TINHAT_JOINSOUND_TEST=1 ./tinhat
pub fn headless_test() -> bool {
    let mut fails = 0;
    let mut check = |label: &str, ok: bool| {
        eprintln!("[joinsound] {} {label}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            fails += 1;
        }
    };
    let now = Instant::now();
    let url = "mxc://example.com/abc";

    check("plays when set, room on, never played", may_play(url, true, None, now).is_ok());
    check("silent when no sound is set", may_play("", true, None, now).is_err());
    check("silent when the room turned them off", may_play(url, false, None, now).is_err());
    check(
        "rejoin inside the cooldown is suppressed",
        may_play(url, true, Some(now - Duration::from_secs(5)), now).is_err(),
    );
    check(
        "plays again after the cooldown",
        may_play(url, true, Some(now - COOLDOWN - Duration::from_secs(1)), now).is_ok(),
    );
    check(
        "whitespace-only url counts as unset",
        may_play("   ", true, None, now).is_err(),
    );
    eprintln!("[joinsound] VERDICT: {}", if fails == 0 { "PASS" } else { "FAIL" });
    fails == 0
}

#[cfg(test)]
mod tests {
    /// sec-night 2026-10-27: what a join sound may play is the soundboard's rule (`soundRoute`), nothing else.
    #[test]
    fn join_sound_plays_only_what_the_soundboard_plays() {
        let base = "https://chat.example.net/api";
        let play = |u: &str| crate::soundboard::clip_url_with(u, base);
        assert_eq!(play("https://chat.example.net/api/soundboard/clip?id=0123abcd"), "https://chat.example.net/api/soundboard/clip?id=0123abcd");
        assert!(play("https://www.myinstants.com/media/sounds/a.mp3").starts_with("https://chat.example.net/api/soundboard/clip?url="));
        assert!(play("https://peer.example.org/api/soundboard/clip?id=1").starts_with("https://chat.example.net/api/soundboard/clip?url="));
        for bad in ["file:///etc/passwd", "smb://host.example/share/a.mp3", "http://evil.example/a.mp3", "https://evil.example/a.mp3", "mxc://example.com/abc", "data:audio/mp3;base64,AA=="] {
            assert_eq!(play(bad), "", "{bad} must not play");
        }
    }
}
