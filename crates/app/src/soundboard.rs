//! Soundboard data + search layer — a port of the Tin Hat web fork's soundboard.
//!
//! TWO sound stores:
//!   • ROOM sounds — one `dev.tinhat.soundboard` custom room-STATE event per sound
//!     (state_key = a fresh UUIDv4), content `{ name, sourceUrl, emoji?, duration }`
//!     where `duration` is WHOLE SECONDS as an INTEGER (canonical-json rejects floats).
//!     DELETE = write an empty `{}` content to that state_key (a tombstone), skipped on
//!     read. Best-effort PL-lower via `appstate::ensure_writable` (tolerates M_FORBIDDEN).
//!   • PERSONAL sounds — a single GLOBAL account-data event
//!     `dev.tinhat.soundboard_personal`, content `{ sounds: [{name,sourceUrl,emoji?,duration}] }`.
//!     Audio is re-fetched from `sourceUrl` at play time (no blobs stored).
//!
//! SEARCH — `proxy::get_json("/soundboard/search?q=…")` → myinstants-style
//! `[{name,url,…}]` (third-party keys stay on the server).
//!
//! IN-CALL PLAYBACK: `play_sound` publishes the Phase-3 2nd audio track via the streamer's
//! `play_url_in_call` (the sound is fetched/decoded by gst and rides the existing call's E2EE
//! key; it auto-unpublishes on EOS). When not in a call it's a SILENT no-op (no error
//! surfaced) — the user still hears the sound via QML's local QtMultimedia MediaPlayer, which
//! is what makes a soundboard sound audible regardless of call state. The call-pipeline
//! cluster lives in call.rs / streamer.

use core::pin::Pin;
use std::sync::Mutex;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use matrix_sdk::ruma::events::GlobalAccountDataEventType;
use matrix_sdk::ruma::RoomId;
use matrix_sdk::Room;
use serde_json::Value;
use tokio::task::JoinHandle;

/// Custom room-state event type: one per room sound (state_key = UUIDv4).
const ROOM_EVENT_TYPE: &str = "dev.tinhat.soundboard";
/// Global account-data event type holding all of MY personal sounds.
const PERSONAL_EVENT_TYPE: &str = "dev.tinhat.soundboard_personal";

#[cxx_qt::bridge]
pub mod qobject {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name] // snake_case Rust -> camelCase QML
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, room_sounds_json)] //     QML: roomSoundsJson — [{stateKey,name,sourceUrl,emoji,duration,setBy}]
        #[qproperty(QString, personal_sounds_json)] // QML: personalSoundsJson — [{name,sourceUrl,emoji,duration}]
        #[qproperty(QString, search_results_json)] //  QML: searchResultsJson — [{name,url,...}]
        #[qproperty(bool, searching)] //               QML: searching
        #[qproperty(QString, error)] //                QML: error — "" or a user-facing message
        #[qproperty(bool, call_playing)] //            QML: callPlaying — the "sound" 2nd track is live in the call (0.8.21)
        #[qproperty(bool, uploading)] //               QML: uploading — an own-clip upload is in flight (0.8.22)
        type SoundboardController = super::SoundboardControllerRust;

        /// QML: refresh(roomId) — (re)load this room's sounds + my personal sounds.
        #[qinvokable]
        fn refresh(self: Pin<&mut SoundboardController>, room_id: &QString);
        /// QML: addRoomSound(roomId, name, sourceUrl, emoji, durationSecs) — write a new
        /// room sound (fresh UUID state_key); duration is truncated to whole seconds.
        #[qinvokable]
        fn add_room_sound(
            self: Pin<&mut SoundboardController>,
            room_id: &QString,
            name: &QString,
            source_url: &QString,
            emoji: &QString,
            duration_secs: i32,
        );
        /// QML: removeRoomSound(roomId, stateKey) — tombstone (empty content) a room sound.
        #[qinvokable]
        fn remove_room_sound(
            self: Pin<&mut SoundboardController>,
            room_id: &QString,
            state_key: &QString,
        );
        /// QML: addPersonalSound(name, sourceUrl, emoji, durationSecs) — append to my
        /// personal account-data sound list.
        #[qinvokable]
        fn add_personal_sound(
            self: Pin<&mut SoundboardController>,
            name: &QString,
            source_url: &QString,
            emoji: &QString,
            duration_secs: i32,
        );
        /// QML: removePersonalSound(index) — drop the i-th personal sound.
        #[qinvokable]
        fn remove_personal_sound(self: Pin<&mut SoundboardController>, index: i32);
        /// QML: search(query) — debounced (450ms, aborts prior in-flight) myinstants
        /// search via the server's Tin Hat API; empty query clears results.
        #[qinvokable]
        fn search(self: Pin<&mut SoundboardController>, query: &QString);
        /// QML: playSound(roomId, sourceUrl) — publish a sound into the ACTIVE call via the
        /// Phase-3 2nd audio track (stop-then-replace any current sound; auto-stops on EOS).
        /// This is the CALL-PUBLISH path only (so peers hear it); LOCAL/self playback is done
        /// independently in QML via a QtMultimedia MediaPlayer. When not in a call this is a
        /// silent no-op (no error) — local playback still makes the sound audible to the user.
        #[qinvokable]
        fn play_sound(self: Pin<&mut SoundboardController>, room_id: &QString, source_url: &QString);
        /// QML: stopSound() — unpublish the current sound's 2nd audio track from the call so
        /// peers stop hearing it (the local MediaPlayer is stopped separately in QML).
        /// Idempotent no-op if nothing is playing / not in a call.
        #[qinvokable]
        fn stop_sound(self: Pin<&mut SoundboardController>);
        /// QML: clipUrl(sourceUrl) — the URL to actually PLAY (local preview and call
        /// publish alike): myinstants sounds go through the server's clip cache, anything
        /// else is returned as-is. See `clip_url_for`.
        #[qinvokable]
        fn clip_url(self: &SoundboardController, source_url: &QString) -> QString;
        /// QML: resolveClip(sourceUrl) — server 0.3.4 (security review H4): the URL a player that can't send a header
        /// (QtMultimedia, GStreamer) should play, resolved off the UI thread: for a myinstants sound a signed link from
        /// the server (`/soundboard/clip/sign`), else what clipUrl gives. Answers with clipResolved(sourceUrl, playUrl)
        /// ("" = this sound never plays: M9).
        #[qinvokable]
        fn resolve_clip(self: Pin<&mut SoundboardController>, source_url: &QString);
        /// The answer to resolveClip.
        #[qsignal]
        fn clip_resolved(self: Pin<&mut SoundboardController>, source_url: QString, play_url: QString);
        /// QML: padSounds(personalSoundsJson, max) — widgets W2 (the Soundboard pad, WIDGETS.md §22): the favourites as the
        /// pad's buttons `[{name, url, emoji}]` (tinhat_widgets::soundpad::pad_sounds on `{sounds: …}`).
        #[qinvokable]
        fn pad_sounds(self: &SoundboardController, sounds_json: &QString, max: i32) -> QString;
        /// QML: refreshPersonal() — widgets W2: re-read only MY sounds (the favourites) — the pad has no room to pass
        /// (refresh(roomId) would also replace the panel's room sounds).
        #[qinvokable]
        fn refresh_personal(self: Pin<&mut SoundboardController>);
        /// QML: padGate(inCall, callPermsJson) — `{allowed, hint}`: the advisory `use_soundboard` permission in a call
        /// and the one line the panel and the pad show (soundpad::pad_gate).
        #[qinvokable]
        fn pad_gate(self: &SoundboardController, in_call: bool, call_perms_json: &QString) -> QString;
        /// QML: refreshState() — poll whether the call-side sound track is still live
        /// (mirrors into `callPlaying`; called from the 120 ms mirror timer).
        #[qinvokable]
        fn refresh_state(self: Pin<&mut SoundboardController>);
        /// QML: uploadFile(fileUrl, name) — upload an own clip (mp3/ogg/wav/m4a/webm, ≤3 MB) to
        /// the server's soundboard service and add it to MY sounds (favourites) on success.
        #[qinvokable]
        fn upload_file(self: Pin<&mut SoundboardController>, file_url: &QString, name: &QString);
    }

    impl cxx_qt::Threading for SoundboardController {}
}

#[derive(Default)]
pub struct SoundboardControllerRust {
    room_sounds_json: QString,
    personal_sounds_json: QString,
    search_results_json: QString,
    searching: bool,
    error: QString,
    call_playing: bool,
    uploading: bool,
    // Most-recent in-flight search task, so a new keystroke aborts the previous (debounce).
    inflight: Mutex<Option<JoinHandle<()>>>,
}

impl qobject::SoundboardController {
    pub fn refresh(self: Pin<&mut Self>, room_id: &QString) {
        let room_id = room_id.to_string();
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            // Room sounds come from the LOCAL state store (no network) — push them IMMEDIATELY
            // so the panel paints existing room boards even if the personal-sounds account-data
            // fetch below is slow, errors, or the homeserver is briefly unreachable. (Combining
            // both pushes held the already-loaded room sounds hostage to that network call.)
            let room_json = load_room_sounds_json(&room_id).await;
            let qt_room = qt.clone();
            let _ = qt_room.queue(move |mut o| {
                o.as_mut().set_room_sounds_json(QString::from(&room_json));
            });
            // Personal sounds need a homeserver round-trip — surface them independently.
            let personal_json = load_personal_sounds_json().await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_personal_sounds_json(QString::from(&personal_json));
            });
        });
    }

    pub fn add_room_sound(
        mut self: Pin<&mut Self>,
        room_id: &QString,
        name: &QString,
        source_url: &QString,
        emoji: &QString,
        duration_secs: i32,
    ) {
        let room_id = room_id.to_string();
        let name = name.to_string();
        let source_url = source_url.to_string();
        let emoji = emoji.to_string();
        let duration = duration_secs.max(0) as u64;
        self.as_mut().set_error(QString::default());
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            if let Err(e) = do_add_room_sound(&room_id, &name, &source_url, &emoji, duration).await {
                eprintln!("[soundboard] add room sound failed: {e}");
                let _ = qt.queue(move |mut o| {
                    o.as_mut().set_error(QString::from("Couldn't add the sound"));
                });
                return;
            }
            let room_json = load_room_sounds_json(&room_id).await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_room_sounds_json(QString::from(&room_json));
            });
        });
    }

    pub fn remove_room_sound(self: Pin<&mut Self>, room_id: &QString, state_key: &QString) {
        let room_id = room_id.to_string();
        let state_key = state_key.to_string();
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            if let Err(e) = do_remove_room_sound(&room_id, &state_key).await {
                eprintln!("[soundboard] remove room sound failed: {e}");
            }
            let room_json = load_room_sounds_json(&room_id).await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_room_sounds_json(QString::from(&room_json));
            });
        });
    }

    pub fn add_personal_sound(
        mut self: Pin<&mut Self>,
        name: &QString,
        source_url: &QString,
        emoji: &QString,
        duration_secs: i32,
    ) {
        let name = name.to_string();
        let source_url = source_url.to_string();
        let emoji = emoji.to_string();
        let duration = duration_secs.max(0) as u64;
        self.as_mut().set_error(QString::default());
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            if let Err(e) = do_add_personal_sound(&name, &source_url, &emoji, duration).await {
                eprintln!("[soundboard] add personal sound failed: {e}");
                let _ = qt.queue(move |mut o| {
                    o.as_mut().set_error(QString::from("Couldn't save the sound"));
                });
                return;
            }
            let personal_json = load_personal_sounds_json().await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_personal_sounds_json(QString::from(&personal_json));
            });
        });
    }

    pub fn remove_personal_sound(self: Pin<&mut Self>, index: i32) {
        if index < 0 {
            return;
        }
        let index = index as usize;
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            if let Err(e) = do_remove_personal_sound(index).await {
                eprintln!("[soundboard] remove personal sound failed: {e}");
            }
            let personal_json = load_personal_sounds_json().await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_personal_sounds_json(QString::from(&personal_json));
            });
        });
    }

    pub fn search(mut self: Pin<&mut Self>, query: &QString) {
        let q = query.to_string().trim().to_string();
        self.as_mut().set_error(QString::default());
        if q.is_empty() {
            // Clear results immediately + abort any pending fetch.
            self.as_mut().set_searching(false);
            self.as_mut().set_search_results_json(QString::from("[]"));
            if let Ok(mut g) = self.rust().inflight.lock() {
                if let Some(prev) = g.take() {
                    prev.abort();
                }
            }
            return;
        }
        self.as_mut().set_searching(true);
        let qt = self.qt_thread();
        let handle = crate::bridge::runtime().spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(450)).await; // debounce
            let path = format!("/soundboard/search?q={}", enc(&q));
            let result = crate::proxy::get_json(&path).await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_searching(false);
                match result {
                    Ok(v) => {
                        let json = normalize_search_results(&v);
                        o.as_mut().set_search_results_json(QString::from(&json));
                    }
                    Err(e) => {
                        eprintln!("[soundboard] search: {e}");
                        o.as_mut().set_error(QString::from("Couldn't search sounds"));
                        o.as_mut().set_search_results_json(QString::from("[]"));
                    }
                }
            });
        });
        // Abort the previous in-flight search (debounce), keep the new handle.
        if let Ok(mut g) = self.rust().inflight.lock() {
            if let Some(prev) = g.replace(handle) {
                prev.abort();
            }
        }
    }

    /// Publish a sound into the active call's 2nd audio track (Phase-3) so PEERS hear it.
    /// This is the CALL-PUBLISH side only — the user's own (self-audible) playback is handled
    /// independently in QML via a QtMultimedia MediaPlayer, so it is NOT gated on being in a
    /// call. When there's no active call this is a SILENT no-op (no error surfaced): the local
    /// QML playback still makes the sound audible to the user. Resolves the active call via the
    /// streamer's process-global participant (set on join, cleared on leave). Overlap policy is
    /// stop-any-current-then-play (handled in `play_url_in_call`).
    pub fn play_sound(mut self: Pin<&mut Self>, room_id: &QString, source_url: &QString) {
        // 0.8.21: publish the CACHED clip (see clip_url_for), never the myinstants URL.
        let src = source_url.to_string();
        let url = clip_url_for(&src);
        let room = room_id.to_string();
        self.as_mut().set_error(QString::default());
        if url.trim().is_empty() {
            if !source_url.to_string().trim().is_empty() {
                eprintln!("[soundboard] playSound refused a sound that isn't on the server or myinstants");
                self.as_mut().set_error(QString::from(FOREIGN_SOUND_NOTE));
            }
            return;
        }
        let qt = self.qt_thread();
        // this request replaces any earlier one still on its way (Astra follow-up: a slow /clip/sign answer for A must
        // not start A after the person stopped it or pressed B)
        let ticket = play_gate().begin();
        crate::bridge::runtime().spawn(async move {
            if !crate::call_ctl::in_active_call().await {
                // Not in a call: the call-publish is a no-op, but QML's local MediaPlayer
                // already plays the sound for the user. No error — this is expected.
                eprintln!("[soundboard] playSound(room={room}): not in a call — local-only");
                return;
            }
            // server 0.3.4: GStreamer can't send the bearer — a myinstants sound plays from a signed link
            let url = playable_clip_url(&src).await;
            if url.is_empty() {
                return;
            }
            // one start at a time; the newest request wins; a stop or a newer press while this one was starting
            // stops it again right after
            let _turn = play_gate().turn.lock().await;
            if !play_gate().current(ticket) {
                eprintln!("[soundboard] playSound: stopped or replaced while its link was fetched — not published");
                return;
            }
            if !crate::call_ctl::in_active_call().await {
                return; // left the call meanwhile
            }
            if let Err(e) = tinhat_streamer::play_url_in_call(&url).await {
                eprintln!("[soundboard] playSound (call publish) failed: {e}");
                let _ = qt.queue(|mut o| {
                    o.as_mut().set_error(QString::from("Couldn't play the sound into the call"));
                });
            } else if !play_gate().current(ticket) {
                tinhat_streamer::stop_url_in_call();
            }
        });
    }

    pub fn refresh_personal(self: Pin<&mut Self>) {
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let personal_json = load_personal_sounds_json().await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_personal_sounds_json(QString::from(&personal_json));
            });
        });
    }

    pub fn pad_sounds(&self, sounds_json: &QString, max: i32) -> QString {
        let list = serde_json::from_str::<Value>(&sounds_json.to_string()).unwrap_or(Value::Null);
        let content = serde_json::json!({ "sounds": list });
        let out: Vec<Value> = tinhat_widgets::soundpad::pad_sounds(&content, max.max(0) as usize).iter().map(|x| x.to_json()).collect();
        QString::from(&Value::Array(out).to_string())
    }

    pub fn pad_gate(&self, in_call: bool, call_perms_json: &QString) -> QString {
        let perms = serde_json::from_str::<Value>(&call_perms_json.to_string()).unwrap_or(Value::Null);
        let (allowed, hint) = tinhat_widgets::soundpad::pad_gate(in_call, &perms);
        QString::from(&serde_json::json!({ "allowed": allowed, "hint": hint }).to_string())
    }

    /// Unpublish the current sound's 2nd audio track from the active call (so peers stop
    /// hearing it). The local self-playback MediaPlayer is stopped separately in QML. Cheap +
    /// idempotent: a no-op when nothing is playing or not in a call.
    pub fn stop_sound(self: Pin<&mut Self>) {
        play_gate().cancel(); // a play still fetching its link must not start after this
        tinhat_streamer::stop_url_in_call();
    }

    pub fn clip_url(&self, source_url: &QString) -> QString {
        QString::from(&clip_url_for(&source_url.to_string()))
    }

    pub fn resolve_clip(self: Pin<&mut Self>, source_url: &QString) {
        let src = source_url.to_string();
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let play = playable_clip_url(&src).await;
            let _ = qt.queue(move |mut o| o.as_mut().clip_resolved(QString::from(&src), QString::from(&play)));
        });
    }

    pub fn upload_file(mut self: Pin<&mut Self>, file_url: &QString, name: &QString) {
        let raw = file_url.to_string();
        let name = name.to_string().trim().chars().take(80).collect::<String>();
        // FileDialog / DropArea hand us file:// URLs; a bare path is accepted too.
        let path = if raw.starts_with("file:") {
            url::Url::parse(&raw).ok().and_then(|u| u.to_file_path().ok())
        } else {
            Some(std::path::PathBuf::from(&raw))
        };
        let Some(path) = path else {
            self.as_mut().set_error(QString::from("That file couldn't be read"));
            return;
        };
        self.as_mut().set_error(QString::default());
        self.as_mut().set_uploading(true);
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let result = upload_clip(&path, &name).await;
            let personal_json = if result.is_ok() { Some(load_personal_sounds_json().await) } else { None };
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_uploading(false);
                match result {
                    Ok(n) => eprintln!("[soundboard] uploaded + added to favourites: {n}"),
                    Err(e) => {
                        eprintln!("[soundboard] upload failed: {e}");
                        o.as_mut().set_error(QString::from(&format!("Upload failed: {e}")));
                    }
                }
                if let Some(j) = personal_json {
                    o.as_mut().set_personal_sounds_json(QString::from(&j));
                }
            });
        });
    }

    pub fn refresh_state(mut self: Pin<&mut Self>) {
        let live = tinhat_streamer::is_sound_playing();
        if *self.call_playing() != live {
            self.as_mut().set_call_playing(live);
        }
    }
}

// ---------------------------------------------------------------------------
// Data layer (reusable by the headless verifier).
// ---------------------------------------------------------------------------

async fn room_by_id(room_id: &str) -> Option<Room> {
    let client = crate::session::client()?;
    let rid = RoomId::parse(room_id).ok()?;
    client.get_room(&rid)
}

/// One sound's content object, validated. Returns `None` for tombstones (no name/url).
fn read_sound_content(content: &Value) -> Option<(String, String, String, u64)> {
    let name = content.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let url = content.get("sourceUrl").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if name.is_empty() || url.is_empty() {
        return None; // tombstone / cleared / invalid
    }
    let emoji = content.get("emoji").and_then(|v| v.as_str()).unwrap_or("").to_string();
    // duration is whole seconds; tolerate a float in old/foreign data by truncating.
    let duration = content
        .get("duration")
        .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f.max(0.0) as u64)))
        .unwrap_or(0);
    Some((name, url, emoji, duration))
}

/// All of a room's (non-tombstone) sounds as JSON `[{stateKey,name,sourceUrl,emoji,duration,setBy}]`.
pub async fn load_room_sounds_json(room_id: &str) -> String {
    let Some(room) = room_by_id(room_id).await else {
        return "[]".to_string();
    };
    let mut out: Vec<Value> = Vec::new();
    for (state_key, sender, content) in
        crate::appstate::get_state_all(&room, ROOM_EVENT_TYPE).await
    {
        if let Some((name, url, emoji, duration)) = read_sound_content(&content) {
            // `addedAt` (epoch-millis) — the web fork's sort key; tolerate a float or absence.
            let added_at = content
                .get("addedAt")
                .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f.max(0.0) as u64)))
                .unwrap_or(0);
            out.push(serde_json::json!({
                "stateKey": state_key,
                "name": name,
                "sourceUrl": url,
                "emoji": emoji,
                "duration": duration,
                "addedAt": added_at,
                "setBy": sender, // ownership from the Matrix event sender (always present)
            }));
        }
    }
    // Match the web fork's display order: newest first (by addedAt desc). Entries missing
    // addedAt (older/foreign) fall to 0 → sort last, same as the web fork.
    out.sort_by(|a, b| {
        let ka = a.get("addedAt").and_then(|v| v.as_u64()).unwrap_or(0);
        let kb = b.get("addedAt").and_then(|v| v.as_u64()).unwrap_or(0);
        kb.cmp(&ka)
    });
    serde_json::to_string(&out).unwrap_or_else(|_| "[]".to_string())
}

/// Write a new room sound (fresh UUIDv4 state_key). `duration` is WHOLE SECONDS.
pub async fn do_add_room_sound(
    room_id: &str,
    name: &str,
    source_url: &str,
    emoji: &str,
    duration: u64,
) -> matrix_sdk::Result<()> {
    let Some(room) = room_by_id(room_id).await else {
        return Ok(()); // room vanished between QML call + task (fail-soft, matches activity.rs)
    };
    crate::appstate::ensure_writable(&room, &[ROOM_EVENT_TYPE]).await;
    let state_key = uuid_v4();
    let mut content = serde_json::json!({
        "name": name.trim(),
        "sourceUrl": source_url.trim(),
        "duration": duration, // integer seconds — NOT a float (canonical-json)
        // The web fork writes these two and relies on them: it sorts the list by `addedAt`
        // (desc) and reads ownership from `addedBy`. Write them for cross-client parity.
        // `addedAt` is whole epoch-millis (integer, never a float → canonical-json safe).
        "addedAt": crate::appstate::now_ms(),
    });
    if let Some(uid) = crate::session::client().and_then(|c| c.user_id().map(|u| u.to_string())) {
        content["addedBy"] = Value::from(uid);
    }
    if !emoji.trim().is_empty() {
        content["emoji"] = Value::from(emoji.trim());
    }
    crate::appstate::send_state(&room, ROOM_EVENT_TYPE, &state_key, content).await
}

/// Tombstone (empty content) a room sound by its state_key.
pub async fn do_remove_room_sound(room_id: &str, state_key: &str) -> matrix_sdk::Result<()> {
    let Some(room) = room_by_id(room_id).await else {
        return Ok(());
    };
    crate::appstate::send_state(&room, ROOM_EVENT_TYPE, state_key, serde_json::json!({})).await
}

/// Fetch my personal-sound list from global account-data (server-fetch, so it works even
/// before the store has synced it). Returns the `sounds` array (each validated) as JSON.
pub async fn load_personal_sounds_json() -> String {
    serde_json::to_string(&load_personal_sounds_vec().await).unwrap_or_else(|_| "[]".to_string())
}

/// The RAW `sounds` array from my personal account-data, UNFILTERED (every entry kept,
/// all fields preserved). This is the read side of the write-path read-modify-write, so a
/// single add/remove never silently drops malformed or extra-field entries written by
/// another client. The display/QML path uses the validated `load_personal_sounds_vec`.
async fn load_personal_sounds_raw() -> Vec<Value> {
    let Some(client) = crate::session::client() else {
        return Vec::new();
    };
    let raw = match client
        .account()
        .fetch_account_data(GlobalAccountDataEventType::from(PERSONAL_EVENT_TYPE))
        .await
    {
        Ok(Some(raw)) => raw,
        Ok(None) => return Vec::new(),
        Err(e) => {
            eprintln!("[soundboard] fetch personal failed: {e}");
            return Vec::new();
        }
    };
    let content: Value = match raw.deserialize_as_unchecked::<Value>() {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    content.get("sounds").and_then(|v| v.as_array()).cloned().unwrap_or_default()
}

/// The `sounds` array from my personal account-data, VALIDATED + normalized for display
/// (each entry reduced to `{name,sourceUrl,emoji,duration}`; tombstones/invalid dropped).
/// READ-ONLY view — never write this back (it is lossy); writers use the raw array.
async fn load_personal_sounds_vec() -> Vec<Value> {
    load_personal_sounds_raw()
        .await
        .iter()
        .filter_map(|s| {
            read_sound_content(s).map(|(name, url, emoji, duration)| {
                serde_json::json!({
                    "name": name,
                    "sourceUrl": url,
                    "emoji": emoji,
                    "duration": duration,
                })
            })
        })
        .collect()
}

/// Append a personal sound (read-modify-write the RAW account-data list, so existing
/// entries — including any with extra fields or written by another client — are preserved).
pub async fn do_add_personal_sound(
    name: &str,
    source_url: &str,
    emoji: &str,
    duration: u64,
) -> Result<(), String> {
    let client = crate::session::client().ok_or("not signed in")?;
    let mut sounds = load_personal_sounds_raw().await;
    let mut entry = serde_json::json!({
        "name": name.trim(),
        "sourceUrl": source_url.trim(),
        "duration": duration, // integer seconds
    });
    if !emoji.trim().is_empty() {
        entry["emoji"] = Value::from(emoji.trim());
    }
    sounds.push(entry);
    write_personal_sounds(&client, sounds).await
}

/// Drop the i-th personal sound. `index` is into the DISPLAYED (validated/filtered) list
/// that QML renders, so map it back to a position in the RAW array before removing — that
/// keeps the read-modify-write loss-free (unknown/foreign entries survive) while still
/// removing exactly the sound the user clicked.
pub async fn do_remove_personal_sound(index: usize) -> Result<(), String> {
    let client = crate::session::client().ok_or("not signed in")?;
    let mut sounds = load_personal_sounds_raw().await;
    // Walk the raw array, counting only entries that survive the display filter; the
    // `index`-th surviving entry is the one to drop.
    let mut seen_valid = 0usize;
    let raw_pos = sounds.iter().position(|s| {
        if read_sound_content(s).is_some() {
            if seen_valid == index {
                return true;
            }
            seen_valid += 1;
        }
        false
    });
    let Some(pos) = raw_pos else {
        return Ok(()); // index out of range of the displayed list
    };
    sounds.remove(pos);
    write_personal_sounds(&client, sounds).await
}

async fn write_personal_sounds(client: &matrix_sdk::Client, sounds: Vec<Value>) -> Result<(), String> {
    use matrix_sdk::ruma::events::AnyGlobalAccountDataEventContent;
    use matrix_sdk::ruma::serde::Raw;
    let content = serde_json::json!({ "sounds": sounds });
    let raw: Raw<AnyGlobalAccountDataEventContent> =
        Raw::new(&content).map_err(|e| e.to_string())?.cast_unchecked();
    client
        .account()
        .set_account_data_raw(GlobalAccountDataEventType::from(PERSONAL_EVENT_TYPE), raw)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Minimal RFC-4122 v4 UUID from `rand` (no `uuid` crate in the tree). Lowercase hex,
/// version + variant nibbles set. Good enough for a unique state_key.
fn uuid_v4() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // variant 10
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

/// myinstants base — the proxy returns a relative `mp3Path`; the playable/storable URL is
/// this host + that path (matches the canonical `sourceUrl` the web fork stores).
const MYINSTANTS_BASE: &str = "https://www.myinstants.com";

/// Normalize the proxy's myinstants-style results into a UNIFORM `[{name,url,duration}]`
/// for QML/personal/room storage. The proxy yields `{name, mp3Path}` (relative path), so
/// build a full `url` from `mp3Path` (or a pre-absolute `url`/`sound`/`mp3` field if one is
/// ever present). Entries without a usable URL are dropped.
pub fn normalize_search_results(v: &Value) -> String {
    // Usually a bare array [{name,mp3Path}]; also accept a future object-wrapped response
    // ({results|sounds|data:[...]}) so firming up the proxy shape doesn't silently 0-result.
    let arr = v
        .as_array()
        .cloned()
        .or_else(|| {
            ["results", "sounds", "data"]
                .iter()
                .find_map(|k| v.get(*k).and_then(|x| x.as_array()).cloned())
        })
        .unwrap_or_default();
    let out: Vec<Value> = arr
        .iter()
        .filter_map(|item| {
            let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("").trim().to_string();
            // Prefer an already-absolute url-ish field; else join the relative mp3Path.
            let url = item
                .get("url")
                .or_else(|| item.get("sound"))
                .or_else(|| item.get("mp3"))
                .and_then(|u| u.as_str())
                .filter(|u| u.starts_with("http"))
                .map(|u| u.to_string())
                .or_else(|| {
                    item.get("mp3Path").and_then(|p| p.as_str()).map(|p| {
                        if p.starts_with("http") {
                            p.to_string()
                        } else {
                            format!("{MYINSTANTS_BASE}{}", if p.starts_with('/') { p.to_string() } else { format!("/{p}") })
                        }
                    })
                })?;
            if name.is_empty() && url.is_empty() {
                return None;
            }
            let duration = item
                .get("duration")
                .and_then(|d| d.as_u64().or_else(|| d.as_f64().map(|f| f.max(0.0) as u64)))
                .unwrap_or(0);
            Some(serde_json::json!({
                "name": if name.is_empty() { url.clone() } else { name },
                "url": url,
                "duration": duration,
            }))
        })
        .collect();
    serde_json::to_string(&out).unwrap_or_else(|_| "[]".to_string())
}

/// Minimal RFC-3986 percent-encode for a query value (matches gif.rs).
/// Upload an own clip to the server's soundboard service (`POST /soundboard/upload`, bearer =
/// our Matrix access token, multipart hand-built: the tree's reqwest has no `multipart`
/// feature) and add it to the personal list. Returns the stored name.
async fn upload_clip(path: &std::path::Path, name: &str) -> Result<String, String> {
    let bytes = tokio::fs::read(path).await.map_err(|e| format!("read: {e}"))?;
    if bytes.len() > 3 * 1024 * 1024 {
        return Err("bigger than 3 MB".into());
    }
    let fname = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| "clip".into());
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mime = match ext.as_str() {
        "mp3" => "audio/mpeg", "ogg" | "opus" => "audio/ogg", "wav" => "audio/wav",
        "m4a" => "audio/mp4", "webm" => "audio/webm", _ => "application/octet-stream",
    };
    let name = if name.trim().is_empty() {
        fname.rsplit_once('.').map(|(n, _)| n.to_string()).unwrap_or_else(|| fname.clone())
    } else {
        name.to_string()
    };
    let token = crate::session::client().and_then(|c| c.access_token()).ok_or("not logged in")?;
    let boundary = format!("----Tin HatUpload{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0));
    let mut body: Vec<u8> = Vec::with_capacity(bytes.len() + 512);
    body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\n{name}\r\n").as_bytes());
    body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\nContent-Type: {mime}\r\n\r\n",
        fname.replace('"', "")).as_bytes());
    body.extend_from_slice(&bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let url = format!("{}/soundboard/upload", crate::proxy::base());
    let resp = crate::proxy::http(None) // 10.5.2: `TinHat/<version> (<os>)`
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", format!("multipart/form-data; boundary={boundary}"))
        .body(body)
        .send()
        .await
        .map_err(|e| format!("network: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let msg = serde_json::from_str::<Value>(&text).ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(|s| s.to_string()))
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(msg);
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("bad reply: {e}"))?;
    let clip_url = v.get("url").and_then(|u| u.as_str()).ok_or("no url in reply")?.to_string();
    let stored = v.get("name").and_then(|n| n.as_str()).unwrap_or(&name).to_string();
    do_add_personal_sound(&stored, &clip_url, "", 0).await.map_err(|e| format!("saved on the server but not in your list: {e}"))?;
    Ok(stored)
}

/// The URL a sound is PLAYED from (local preview + call publish). myinstants clips go
/// through the server's clip cache (`/soundboard/clip?url=…`) because (a) since 2026-09
/// myinstants' Cloudflare answers 403 to Qt's FFmpeg backend on Windows — the clicker
/// heard nothing while gst (souphttpsrc) fetched fine for the call — and
/// (b) one cached fetch then serves everyone, forever. 10.5.1: a link on one of our own API
/// names (an uploaded sound, stored as `https://<former name>/api/soundboard/clip?id=…`
/// by 10.5.0 and older) is re-homed onto `proxy::base()` first.
/// M9 (security review 2026-10-26): ANYTHING ELSE IS REFUSED ("" = nothing plays). A room sound's `sourceUrl` is room
/// state any moderator can write; returned as-is it sent every clicker's GStreamer `uridecodebin` (any URI scheme it
/// knows) to a stranger's host, decoded their bytes and published the result into the call. The server's clip cache
/// only fetches myinstants (it answers 403 to any other host), so there is nothing to route a foreign link through.
pub fn clip_url_for(source_url: &str) -> String {
    clip_url_with(&crate::proxy::rebase_url(source_url.trim()), &crate::proxy::base())
}

/// The pure half of [`clip_url_for`]: `s` already re-homed, `base` = the server's API base ("" signed out). The myinstants
/// and relative-path mapping is byte-identical with Tin Hat Web's clipUrlFor (shared/clip.ts).
pub(crate) fn clip_url_with(s: &str, base: &str) -> String {
    let s = s.trim();
    match sound_route(s, base) {
        // the server's clip cache: myinstants clips, and (decided 2026-10-26) another Tin Hat server's uploads — never
        // fetched from there by this app
        SoundRoute::Myinstants | SoundRoute::Foreign if !base.is_empty() => format!("{base}/soundboard/clip?url={}", enc(s)),
        SoundRoute::Own => s.to_string(), // an uploaded sound on this account's own server
        _ => String::new(),
    }
}

/// How a stored sound URL is played (shared rule: member-content.json `soundRoute`, the same order in both apps and
/// the server's clip cache): myinstants (a relative path or anything naming myinstants.com/) → our server's clip cache;
/// an upload on our own server in its exact shape ([`own_api_url`]) → direct; another Tin Hat server's upload
/// ([`foreign_upload`]) → our server's clip cache; anything else → refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SoundRoute {
    Myinstants,
    Own,
    Foreign,
    Refuse,
}

pub(crate) fn sound_route(s: &str, base: &str) -> SoundRoute {
    let s = s.trim();
    if s.is_empty() {
        SoundRoute::Refuse
    } else if s.starts_with('/') || s.contains("myinstants.com/") {
        SoundRoute::Myinstants
    } else if own_api_url(s, base) {
        SoundRoute::Own
    } else if foreign_upload(s, base) {
        SoundRoute::Foreign
    } else {
        SoundRoute::Refuse
    }
}

/// Another Tin Hat server's uploaded sound (decided 2026-10-26): `https://<host>[:443]/api/soundboard/clip?id=<id>` (no
/// other query) or `https://<host>[:443]/api/soundboard/file/<name>`, on a host that isn't our own server's, with no
/// user name, no other port, no fragment, no `%2e` / `%2f` / `%5c` (any case), no white space or controls; the host a
/// DNS name (not an IP address, not a single label). Names as [`own_api_url`]'s (a letter, digit, `_` or `-` first).
fn foreign_upload(s: &str, base: &str) -> bool {
    static ID: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static FILE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let id = ID.get_or_init(|| regex::Regex::new(r"^id=[A-Za-z0-9_-]{1,128}$").unwrap());
    let file = FILE.get_or_init(|| regex::Regex::new(r"^/api/soundboard/file/[A-Za-z0-9_-][A-Za-z0-9_.-]{0,127}$").unwrap());
    let lower = s.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "#", "\\"].iter().any(|b| lower.contains(b)) || s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let Ok(u) = url::Url::parse(s) else { return false };
    if u.scheme() != "https" || !u.username().is_empty() || u.password().is_some() || u.port().is_some() || u.fragment().is_some() {
        return false;
    }
    let Some(url::Host::Domain(host)) = u.host() else { return false };
    if !host.contains('.') {
        return false;
    }
    let ours = url::Url::parse(base).ok().and_then(|b| b.host_str().map(str::to_ascii_lowercase));
    if ours.as_deref() == Some(host.to_ascii_lowercase().as_str()) {
        return false; // our own server: only its exact upload shapes, under the API base (own_api_url)
    }
    match (u.path(), u.query()) {
        ("/api/soundboard/clip", Some(q)) => id.is_match(q),
        (p, None) => file.is_match(p),
        _ => false,
    }
}

/// The part of `s` after this account's own API base (`<base>…`, compared case-insensitively), if it is under it.
fn under_api_base<'a>(s: &'a str, base: &str) -> Option<&'a str> {
    let base = base.trim_end_matches('/');
    if base.is_empty() || !(base.starts_with("https://") || base.starts_with("http://")) {
        return None;
    }
    s.get(..base.len()).filter(|h| h.eq_ignore_ascii_case(base)).map(|_| &s[base.len()..])
}

/// Is `s` an uploaded sound on this account's own API? ONLY `<base>/soundboard/clip?id=<id>` or
/// `<base>/soundboard/file/<name>` (launch-gate's review, 2026-10-26: `%2e%2e`, `%2f`, `%5c` got through a `/../` check,
/// and the bearer / cookies go to whatever path a URL normalises to). Shared rule: member-content.json `ownSounds`.
fn own_api_url(s: &str, base: &str) -> bool {
    static SHAPE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let shape = SHAPE.get_or_init(|| {
        regex::Regex::new(r"^/soundboard/(?:clip\?id=[A-Za-z0-9_-]{1,128}|file/[A-Za-z0-9_-][A-Za-z0-9_.-]{0,127})$").unwrap()
    });
    under_api_base(s, base).is_some_and(|rest| shape.is_match(rest))
}

/// A signed clip link the server handed out: `<base>/soundboard/clip?url=…&exp=<ms>&sig=…` (no fragment, no spaces,
/// no backslash; the path is fixed).
fn own_signed_clip(s: &str, base: &str) -> bool {
    under_api_base(s, base).is_some_and(|rest| {
        rest.starts_with("/soundboard/clip?url=")
            && rest.contains("&exp=")
            && rest.contains("&sig=")
            && !rest.chars().any(|c| c.is_whitespace() || c.is_control() || c == '\\' || c == '#')
    })
}

/// The note when a sound is refused (QML: the soundboard's line, the call publish's error).
pub const FOREIGN_SOUND_NOTE: &str = "This sound isn't on your server or myinstants, so Tin Hat won't play it.";

/// The soundboard's call publishes in order (Astra follow-up, 2026-10-26): each playSound takes a ticket; stopSound and
/// every newer playSound make older tickets stale; a publish goes ahead only if its ticket is still the newest, checked
/// right before it starts and again right after (a stop that came while it was starting stops it). `turn` makes the
/// check-and-start of two presses happen one after the other.
pub(crate) struct PlayGate {
    gen: std::sync::atomic::AtomicU64,
    pub(crate) turn: tokio::sync::Mutex<()>,
}
impl PlayGate {
    const fn new() -> Self {
        Self { gen: std::sync::atomic::AtomicU64::new(0), turn: tokio::sync::Mutex::const_new(()) }
    }
    pub(crate) fn begin(&self) -> u64 {
        self.gen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
    }
    pub(crate) fn cancel(&self) {
        self.gen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    pub(crate) fn current(&self, ticket: u64) -> bool {
        self.gen.load(std::sync::atomic::Ordering::SeqCst) == ticket
    }
}
fn play_gate() -> &'static PlayGate {
    static G: PlayGate = PlayGate::new();
    &G
}

// ── Signed clip links (server 0.3.4, security review H4) ───────────────────────────────────────────────────────────
// A 0.3.4 server fetches a myinstants sound it doesn't have yet only for a signed-in member. QtMultimedia and GStreamer
// can't send a header, so before one of them plays a myinstants sound the app asks
// `GET <api>/soundboard/clip/sign?url=<the same value clip?url= carries>` with the bearer → `{url, exp}` (exp in ms, an
// hour) and plays that link (Tin Hat Web does the same for <audio>). Kept per sound until 5 minutes before it runs
// out. A server without the route (0.3.3 and older: 404) is remembered for 30 minutes and plays today's plain clip URL;
// any other failure plays the plain URL too (a cached clip still plays from it).

/// `(signed url, exp ms)` per `base\nsource`, and the bases whose server has no /clip/sign (until when).
fn sign_cache() -> &'static Mutex<std::collections::HashMap<String, (String, u64)>> {
    static C: std::sync::OnceLock<Mutex<std::collections::HashMap<String, (String, u64)>>> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}
fn sign_unsupported() -> &'static Mutex<std::collections::HashMap<String, u64>> {
    static C: std::sync::OnceLock<Mutex<std::collections::HashMap<String, u64>>> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}
/// A signed link is reused until this long before it runs out.
const SIGN_MARGIN_MS: u64 = 5 * 60 * 1000;

/// The URL QtMultimedia / GStreamer should play for a stored sound: "" (refused, M9), an uploaded sound as stored, a
/// myinstants sound as a signed link when the server gives one, else the plain clip URL.
pub async fn playable_clip_url(source_url: &str) -> String {
    let s = crate::proxy::rebase_url(source_url.trim());
    let base = crate::proxy::base();
    let plain = clip_url_with(&s, &base);
    if plain.is_empty() || !goes_through_clip_cache(&s) {
        return plain;
    }
    match signed_clip_url(s.trim(), &base).await {
        Some(u) => u,
        None => plain,
    }
}

/// The sounds the server's clip cache serves (myinstants, relative paths): the ones that may need a signed link.
fn goes_through_clip_cache(s: &str) -> bool {
    matches!(sound_route(s, &crate::proxy::base()), SoundRoute::Myinstants | SoundRoute::Foreign)
}

async fn signed_clip_url(s: &str, base: &str) -> Option<String> {
    let now = crate::appstate::now_ms();
    let key = format!("{base}\n{s}");
    if let Some((u, exp)) = sign_cache().lock().ok()?.get(&key).cloned() {
        if exp > now + SIGN_MARGIN_MS {
            return Some(u);
        }
    }
    if sign_unsupported().lock().ok()?.get(base).is_some_and(|until| *until > now) {
        return None;
    }
    let token = crate::session::client().and_then(|c| c.access_token())?;
    let url = format!("{base}/soundboard/clip/sign?url={}", enc(s));
    let resp = crate::proxy::http(Some(std::time::Duration::from_secs(8)))
        .get(&url)
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| eprintln!("[soundboard] clip/sign: {e}"))
        .ok()?;
    let status = resp.status().as_u16();
    if status == 404 {
        // a server before 0.3.4: plain clip URLs, as before
        if let Ok(mut m) = sign_unsupported().lock() {
            m.insert(base.to_string(), now + 30 * 60 * 1000);
        }
        return None;
    }
    if status != 200 {
        eprintln!("[soundboard] clip/sign: HTTP {status} — playing the plain clip URL");
        return None;
    }
    let v: Value = serde_json::from_str(&resp.text().await.ok()?).ok()?;
    let (u, exp) = accept_signed_clip(&v, base, now)?;
    if let Ok(mut m) = sign_cache().lock() {
        if m.len() > 500 {
            m.retain(|_, (_, e)| *e > now + SIGN_MARGIN_MS);
        }
        m.insert(key, (u.clone(), exp));
    }
    Some(u)
}

/// The server's `{url, exp}` answer, accepted only when the link is a clip URL on this account's own API (M9: nothing
/// else ever reaches GStreamer) and still has more than [`SIGN_MARGIN_MS`] to run (a former name of the server is
/// re-homed first, as every stored link is).
fn accept_signed_clip(v: &Value, base: &str, now: u64) -> Option<(String, u64)> {
    let u = crate::proxy::rebase_url(v.get("url")?.as_str()?.trim());
    let exp = v.get("exp").and_then(|e| e.as_u64().or_else(|| e.as_f64().filter(|f| *f > 0.0).map(|f| f as u64)))?;
    if !own_signed_clip(&u, base) || exp <= now + SIGN_MARGIN_MS {
        return None;
    }
    Some((u, exp))
}


fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Headless round-trip (no GUI) proving the whole data + search layer against the live
/// homeserver/proxy. On the given non-DM room:
///   1. add a room sound -> get_state_all read-back shows it,
///   2. tombstone delete -> read-back gone,
///   3. set a personal sound via account-data -> read back,
///   4. proxy /soundboard/search round-trip.
/// Prints a VERDICT. Cleans up the personal sound it wrote.
///   TINHAT_SOUNDBOARD_TEST='!nondmroom:server' [TINHAT_SOUNDBOARD_QUERY=airhorn] ./tinhat
pub async fn headless_verify(room_id: &str) {
    let Some(client) = crate::session::client() else {
        eprintln!("[sbtest] not signed in");
        return;
    };
    let mut room_ok = false;
    let mut personal_ok = false;
    let mut search_ok = false;

    // ---- (1)+(2) ROOM sound add -> read -> tombstone -> read ----
    let test_name = "headless test sound";
    let test_url = "https://www.myinstants.com/media/sounds/airhorn.mp3";
    match do_add_room_sound(room_id, test_name, test_url, "📣", 2).await {
        Ok(()) => {
            let _ = client
                .sync_once(crate::bridge::sync_settings().timeout(std::time::Duration::from_secs(10)))
                .await;
            let after_add = load_room_sounds_json(room_id).await;
            let present = after_add.contains(test_name);
            eprintln!("[sbtest] room AFTER ADD present={present}: {after_add}");
            // Find the state_key we just wrote so we can tombstone exactly it.
            let sk = find_state_key(room_id, test_name).await;
            if let Some(sk) = sk {
                let _ = do_remove_room_sound(room_id, &sk).await;
                let _ = client
                    .sync_once(crate::bridge::sync_settings().timeout(std::time::Duration::from_secs(10)))
                    .await;
                let after_del = load_room_sounds_json(room_id).await;
                let gone = !after_del.contains(test_name);
                eprintln!("[sbtest] room AFTER DELETE gone={gone}: {after_del}");
                room_ok = present && gone;
            } else {
                eprintln!("[sbtest] room: couldn't find the state_key we wrote (PL/forbidden?)");
            }
        }
        Err(e) => eprintln!("[sbtest] room add FAILED: {e}"),
    }

    // ---- (3) PERSONAL sound set -> read back ----
    let p_name = "headless personal sound";
    match do_add_personal_sound(p_name, test_url, "🔊", 1).await {
        Ok(()) => {
            let after = load_personal_sounds_json().await;
            personal_ok = after.contains(p_name);
            eprintln!("[sbtest] personal AFTER SET ok={personal_ok}: {after}");
            // Clean up: drop the entry we appended (it's the last one).
            let v = load_personal_sounds_vec().await;
            if let Some(idx) = v.iter().rposition(|s| s.get("name").and_then(|n| n.as_str()) == Some(p_name)) {
                let _ = do_remove_personal_sound(idx).await;
                eprintln!("[sbtest] personal cleaned up");
            }
        }
        Err(e) => eprintln!("[sbtest] personal set FAILED: {e}"),
    }

    // ---- (4) PROXY search round-trip ----
    let q = std::env::var("TINHAT_SOUNDBOARD_QUERY").unwrap_or_else(|_| "airhorn".to_string());
    let path = format!("/soundboard/search?q={}", enc(q.trim()));
    eprintln!("[sbtest] GET {}{path}", crate::proxy::base());
    match crate::proxy::get_json(&path).await {
        Ok(v) => {
            // Validate the NORMALIZED shape QML/storage consume ({name,url}), not the raw
            // proxy shape ({name,mp3Path}) — proves the mp3Path->full-URL mapping too.
            let norm: Value = serde_json::from_str(&normalize_search_results(&v)).unwrap_or(Value::Null);
            let arr = norm.as_array().cloned().unwrap_or_default();
            search_ok = arr
                .first()
                .and_then(|f| f.get("url").and_then(|u| u.as_str()))
                .map(|u| u.starts_with("http"))
                .unwrap_or(false);
            eprintln!("[sbtest] search {} result(s); first(normalized)={:?}", arr.len(), arr.first());
        }
        Err(e) => eprintln!("[sbtest] search FAILED (proxy may not expose /soundboard yet): {e}"),
    }

    eprintln!(
        "[sbtest] VERDICT: {} (room={room_ok} personal={personal_ok} search={search_ok})",
        if room_ok && personal_ok && search_ok { "PASS" } else { "PARTIAL/FAIL" }
    );
}

/// Find the state_key of a room sound by its name (for the headless delete step).
async fn find_state_key(room_id: &str, name: &str) -> Option<String> {
    let room = room_by_id(room_id).await?;
    for (sk, _sender, content) in crate::appstate::get_state_all(&room, ROOM_EVENT_TYPE).await {
        if content.get("name").and_then(|n| n.as_str()) == Some(name) {
            return Some(sk);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://chat.example.net/api";

    /// M9: a sound plays from the server's clip cache (myinstants, relative paths — the web's mapping, byte for byte),
    /// from this account's own server (uploads), or not at all.
    #[test]
    fn sounds_play_only_through_the_server() {
        let mi = "https://www.myinstants.com/media/sounds/airhorn.mp3";
        assert_eq!(
            clip_url_with(mi, BASE),
            "https://chat.example.net/api/soundboard/clip?url=https%3A%2F%2Fwww.myinstants.com%2Fmedia%2Fsounds%2Fairhorn.mp3"
        );
        assert_eq!(clip_url_with("/media/sounds/a.mp3", BASE), "https://chat.example.net/api/soundboard/clip?url=%2Fmedia%2Fsounds%2Fa.mp3");
        // an uploaded sound on our own server plays as stored
        let up = "https://chat.example.net/api/soundboard/clip?id=0123abcd";
        assert_eq!(clip_url_with(up, BASE), up);
        assert_eq!(clip_url_with(&format!("{up} "), BASE), up);
        // everything else: refused
        for bad in [
            "https://attacker.example/x.mp3",
            "http://chat.example.net/api/soundboard/clip?id=1",       // our host, but not https like our base
            "https://chat.example.net/apix/soundboard/clip?id=1",     // not under the api path
            "https://chat.example.net.evil.example/api/x.mp3",
            "https://chat.example.net/api/../../etc/passwd",
            "https://chat.example.net/api/soundboard/clip?id=1\nx",
            "file:///etc/passwd",
            "smb://attacker/share/a.mp3",
            "mxc://example.net/abc",
            "rtsp://attacker/live",
        ] {
            assert_eq!(clip_url_with(bad, BASE), "", "{bad:?}");
        }
        // a link that merely mentions myinstants goes to the clip cache, which fetches only real myinstants clips
        assert!(clip_url_with("https://evil.example/myinstants.com/a.mp3", BASE).starts_with("https://chat.example.net/api/soundboard/clip?url="));
        // signed out: nothing plays
        assert_eq!(clip_url_with(mi, ""), "");
        assert_eq!(clip_url_with(up, ""), "");
        assert_eq!(clip_url_with("", BASE), "");
    }

    /// Astra follow-up: a publish whose ticket went stale (stopped, or a newer press) never starts.
    #[test]
    fn a_stopped_or_replaced_sound_is_not_published() {
        let g = PlayGate::new();
        let a = g.begin();
        assert!(g.current(a));
        let b = g.begin(); // the person pressed B while A's link was on its way
        assert!(!g.current(a) && g.current(b));
        g.cancel(); // then stopped
        assert!(!g.current(b));
        let c = g.begin();
        assert!(g.current(c) && c > b);
    }

    /// The shared routing rule (member-content.json `soundRoute`): myinstants → own → another Tin Hat server → refused,
    /// and what plays.
    #[test]
    fn sound_route_vectors() {
        let v: Value = serde_json::from_str(include_str!("testvectors/member-content.json")).unwrap();
        let cases = v["soundRoute"].as_array().expect("soundRoute cases");
        assert!(cases.len() >= 25);
        let mut fails = Vec::new();
        for c in cases {
            let (url, base, want) = (c["url"].as_str().unwrap(), c["base"].as_str().unwrap(), c["route"].as_str().unwrap());
            let got = match sound_route(url, base) {
                SoundRoute::Myinstants => "myinstants",
                SoundRoute::Own => "own",
                SoundRoute::Foreign => "foreign",
                SoundRoute::Refuse => "refuse",
            };
            if got != want {
                fails.push(format!("{url} under {base}: {got}, want {want}"));
            }
            let play = clip_url_with(url, base.trim_end_matches('/'));
            let want_play = c.get("play").and_then(Value::as_str).unwrap_or("");
            if play != want_play {
                fails.push(format!("{url}: plays {play:?}, want {want_play:?}"));
            }
        }
        assert!(fails.is_empty(), "{}", fails.join("\n"));
    }

    /// The shared own-sound rule (member-content.json `ownSounds`): only the two upload shapes, no encoded dots,
    /// slashes or backslashes.
    #[test]
    fn own_sound_vectors() {
        let v: Value = serde_json::from_str(include_str!("testvectors/member-content.json")).unwrap();
        let cases = v["ownSounds"].as_array().expect("ownSounds cases");
        assert!(cases.len() >= 12);
        for c in cases {
            let (url, base) = (c["url"].as_str().unwrap(), c["base"].as_str().unwrap());
            assert_eq!(own_api_url(url, base), c["own"].as_bool().unwrap(), "{url} under {base}");
        }
    }

    /// Launch-gate's web review #3: the desktop takes a sound stored under one of the server's FORMER API names
    /// (an address that isn't today's API base — e.g. the web app's host when it served the API) as the server's own,
    /// after re-homing it (proxy.rs rebase_onto, the names the server's well-known lists in dev.tinhat.moved_from);
    /// an address on a host the server never named is not re-homed (it's another server's: proxied, never direct).
    #[test]
    fn a_sound_on_a_former_api_name_is_rehomed_then_played() {
        let base = "https://chat.example.net/api";
        let known = vec![base.to_string(), "https://web.example.net/api".to_string()];
        let stored = "https://web.example.net/api/soundboard/clip?id=0123abcd";
        let rehomed = crate::proxy::rebase_onto_for_tests(stored, base, &known);
        assert_eq!(rehomed, "https://chat.example.net/api/soundboard/clip?id=0123abcd");
        assert_eq!(clip_url_with(&rehomed, base), rehomed);
        // a host the server never named is another Tin Hat server: fetched through our own server, never directly
        let stranger = "https://other.example.net/api/soundboard/clip?id=0123abcd";
        assert_eq!(crate::proxy::rebase_onto_for_tests(stranger, base, &known), stranger);
        assert_eq!(clip_url_with(stranger, base), format!("{base}/soundboard/clip?url={}", enc(stranger)));
    }

    /// Server 0.3.4: the signed link the server hands out is taken only as a clip URL on our own API with time left.
    #[test]
    fn a_signed_clip_link_is_ours_and_fresh() {
        let now = 1_790_000_000_000u64;
        let link = "https://chat.example.net/api/soundboard/clip?url=https%3A%2F%2Fwww.myinstants.com%2Fmedia%2Fsounds%2Fa.mp3&exp=1790003600000&sig=0a1b";
        let ok = serde_json::json!({ "url": link, "exp": 1_790_003_600_000u64 });
        assert_eq!(accept_signed_clip(&ok, BASE, now), Some((link.to_string(), 1_790_003_600_000)));
        // a float exp (JavaScript numbers) reads the same
        let f = serde_json::json!({ "url": link, "exp": 1_790_003_600_000f64 });
        assert_eq!(accept_signed_clip(&f, BASE, now).map(|x| x.1), Some(1_790_003_600_000));
        for bad in [
            serde_json::json!({ "url": "https://evil.example/soundboard/clip?url=x&exp=1&sig=2", "exp": 1_790_003_600_000u64 }),
            serde_json::json!({ "url": "https://chat.example.net/api/soundboard/other?x=1", "exp": 1_790_003_600_000u64 }),
            serde_json::json!({ "url": link, "exp": now + 60_000 }),          // runs out within the margin
            serde_json::json!({ "url": link }),
            serde_json::json!({ "exp": 1_790_003_600_000u64 }),
            serde_json::json!("x"),
        ] {
            assert_eq!(accept_signed_clip(&bad, BASE, now), None, "{bad}");
        }
        // only clip-cache sounds ask for a link; an uploaded sound plays as stored
        assert_eq!(sound_route("https://www.myinstants.com/media/sounds/a.mp3", BASE), SoundRoute::Myinstants);
        assert_eq!(sound_route("/media/sounds/a.mp3", BASE), SoundRoute::Myinstants);
        assert_eq!(sound_route("https://chat.example.net/api/soundboard/clip?id=1", BASE), SoundRoute::Own);
        assert_eq!(sound_route("https://peer.example.org/api/soundboard/clip?id=1", BASE), SoundRoute::Foreign);
    }
}
