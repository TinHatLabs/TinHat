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
        let url = clip_url_for(&source_url.to_string());
        let room = room_id.to_string();
        self.as_mut().set_error(QString::default());
        if url.trim().is_empty() {
            return;
        }
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            if !crate::call_ctl::in_active_call().await {
                // Not in a call: the call-publish is a no-op, but QML's local MediaPlayer
                // already plays the sound for the user. No error — this is expected.
                eprintln!("[soundboard] playSound(room={room}): not in a call — local-only");
                return;
            }
            if let Err(e) = tinhat_streamer::play_url_in_call(&url).await {
                eprintln!("[soundboard] playSound (call publish) failed: {e}");
                let _ = qt.queue(|mut o| {
                    o.as_mut().set_error(QString::from("Couldn't play the sound into the call"));
                });
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
        tinhat_streamer::stop_url_in_call();
    }

    pub fn clip_url(&self, source_url: &QString) -> QString {
        QString::from(&clip_url_for(&source_url.to_string()))
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
/// by 10.5.0 and older) is re-homed onto `proxy::base()` first. Anything else is returned as-is.
pub fn clip_url_for(source_url: &str) -> String {
    let s = crate::proxy::rebase_url(source_url.trim());
    if s.is_empty() || !(s.starts_with('/') || s.contains("myinstants.com/")) {
        return s;
    }
    format!("{}/soundboard/clip?url={}", crate::proxy::base(), enc(&s))
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
