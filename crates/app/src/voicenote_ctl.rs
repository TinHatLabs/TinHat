//! VoiceNoteController — voice notes, the desktop half (docs/VOICE-NOTES.md). Record (the streamer's recorder:
//! tinhat_streamer::voicenote), review, send (RoomController.sendVoiceNote: one local echo, upload, voice_content,
//! send_raw with the draft's transaction id).
//!
//! The state is process-wide (one microphone, one recorder): a [`Machine`] holds one draft per room, in memory only;
//! every view (the main conversation, a floating chat) is a window onto it — `show(room)` + `snapshot()` polled by the
//! recorder row while it is visible. States `idle → requesting → recording → finalizing → review → sending → idle`;
//! `sending` failed → `review` with the error. Each recording has an id; every async answer (the recorder's start, the
//! EOS, a send result) is checked against the current draft of the same account + room, so a late answer after Cancel,
//! a room switch or a call join is dropped. A retry reuses the finished upload and the same transaction id.
//!
//! Drafts go away on sign-out / account switch / leaving the room (files deleted); a crash leaves `voice-*.ogg` in the
//! media cache, swept at the first controller of the next start.

use core::pin::Pin;
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;
use serde_json::json;
use tinhat_widgets::voicenote::{clock, VOICE_MAX_MS, VOICE_MIN_MS, VOICE_WARN_LEFT_MS};

#[cxx_qt::bridge]
pub mod qobject {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(bool, probe)] //       QML: probe — TINHAT_GUI_VOICEPROBE=1: ChatBubble logs each voice note's player load (interop gate)
        #[qproperty(QString, auto_gate)] // QML: autoGate — TINHAT_GUI_VOICENOTE="<roomId>,<secs>": record + send one note, log it, quit (gate)
        type VoiceNoteController = super::VoiceNoteControllerRust;

        /// Every action names its room (review r1 #7): a room the view had not synced yet is synced first, so an action
        /// never lands in the previously shown room.
        /// QML: show(roomId) — the room this view shows ("" = none / the view closes). The view that started a recording moving to
        /// another room stops it into review (the draft waits in its room).
        #[qinvokable]
        fn show(self: Pin<&mut VoiceNoteController>, room_id: &QString);
        /// QML: snapshot() — JSON {state, elapsedMs, clock, warn, level, levels[], waveform[], durationMs, durationClock,
        /// fileUrl, error, txn, inCall, events[{kind, text, eventId?}]} for the shown room (events are consumed).
        #[qinvokable]
        fn snapshot(self: Pin<&mut VoiceNoteController>) -> QString;
        /// QML: start() — record a note in the shown room (click to start).
        #[qinvokable]
        fn start(self: Pin<&mut VoiceNoteController>, room_id: &QString);
        /// QML: stop() — recording → review.
        #[qinvokable]
        fn stop(self: Pin<&mut VoiceNoteController>, room_id: &QString);
        /// QML: cancel() — discard the recording (no confirmation: nothing was sent).
        #[qinvokable]
        fn cancel(self: Pin<&mut VoiceNoteController>, room_id: &QString);
        /// QML: send() — review → sending (recording: stop, then send). The request comes back through takeSendRequest.
        #[qinvokable]
        fn send(self: Pin<&mut VoiceNoteController>, room_id: &QString);
        /// QML: takeSendRequest() — the shown room's pending send ({room, path, durationMs, waveform, size, txn}, once;
        /// "" = none): the view hands it to room.sendVoiceNote.
        #[qinvokable]
        fn take_send_request(self: Pin<&mut VoiceNoteController>, room_id: &QString) -> QString;
        /// QML: deleteDraft() — discard the reviewed note (its file, its upload); returns its txn ("" = never sent) so the
        /// view can drop a failed echo row with room.discardEcho.
        #[qinvokable]
        fn delete_draft(self: Pin<&mut VoiceNoteController>, room_id: &QString) -> QString;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VnState {
    Requesting,
    Recording,
    Finalizing,
    Review,
    Sending,
}

impl VnState {
    pub fn as_str(self) -> &'static str {
        match self {
            VnState::Requesting => "requesting",
            VnState::Recording => "recording",
            VnState::Finalizing => "finalizing",
            VnState::Review => "review",
            VnState::Sending => "sending",
        }
    }
    /// the recorder holds (or is about to hold) the microphone
    pub fn capturing(self) -> bool {
        matches!(self, VnState::Requesting | VnState::Recording | VnState::Finalizing)
    }
}

/// One room's voice draft.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub id: u64,
    pub room: String,
    pub owner: u64,
    pub state: VnState,
    pub path: String,
    pub samples: u64,
    pub duration_ms: u64,
    pub waveform: Vec<u32>,
    pub size: u64,
    /// the send's transaction id: minted at the first Send, kept for every retry ("" = never sent)
    pub txn: String,
    pub error: String,
    /// Send pressed while recording: send as soon as the note is finalized
    pub send_after_stop: bool,
    /// a send request waits for the view (takeSendRequest)
    pub req_pending: bool,
    /// review r2 #4: the draft's send token — cancelled the moment the draft goes (discard / delete / sign-out / another
    /// account); the send's preparation and send_raw race it
    pub token: SendToken,
}

/// A per-draft cancellation token (cheap to clone; every clone sees the cancel).
#[derive(Clone, Default)]
pub struct SendToken(std::sync::Arc<(std::sync::atomic::AtomicBool, tokio::sync::Notify)>);
impl SendToken {
    pub fn cancel(&self) {
        self.0 .0.store(true, std::sync::atomic::Ordering::SeqCst);
        self.0 .1.notify_waiters();
    }
    pub fn is_cancelled(&self) -> bool {
        self.0 .0.load(std::sync::atomic::Ordering::SeqCst)
    }
    /// Resolves once cancelled (immediately if it already is).
    pub async fn cancelled(&self) {
        loop {
            let n = self.0 .1.notified();
            let mut n = std::pin::pin!(n);
            n.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            n.await;
        }
    }
}
impl PartialEq for SendToken {
    fn eq(&self, o: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &o.0)
    }
}
impl std::fmt::Debug for SendToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SendToken(cancelled={})", self.is_cancelled())
    }
}

/// What a finished recording becomes.
#[derive(Debug, Clone, PartialEq)]
pub enum Finalized {
    Review,
    /// review, then straight on to sending (Send pressed while recording)
    Send,
    /// shorter than 0.5 s: discarded (the caller deletes the file and says "Too short to send")
    TooShort(String),
    /// nothing captured that can be played: discarded with "The recording failed"
    Failed(String),
    /// not the current draft any more (cancelled, another account): the caller deletes the file
    Stale(String),
}

/// What a view's move does to the recording it started.
#[derive(Debug, Clone, PartialEq)]
pub enum Moved {
    /// stop into review
    Stop(u64),
    /// cancel the start (the draft is gone)
    Cancel(u64),
}

/// The pure state machine (unit-tested): drafts per room, one recording at a time, stale answers dropped.
#[derive(Debug, Default)]
pub struct Machine {
    pub account: String,
    next: u64,
    pub drafts: BTreeMap<String, Draft>,
    /// review r2 #4: the finished uploads per txn live HERE, under the same lock as the drafts — inserting one (only
    /// while its draft may still send) and discarding the draft (which drops it) can never interleave
    pub uploads: std::collections::HashMap<String, (tinhat_widgets::voicenote::VoiceMedia, u64)>,
}

impl Machine {
    /// A different account signed in: every draft goes (returned, so their files are deleted).
    pub fn set_account(&mut self, account: &str) -> Vec<Draft> {
        if self.account == account {
            return vec![];
        }
        self.account = account.to_string();
        self.clear()
    }
    pub fn clear(&mut self) -> Vec<Draft> {
        self.uploads.clear();
        let all: Vec<Draft> = std::mem::take(&mut self.drafts).into_values().collect();
        for d in &all {
            d.token.cancel();
        }
        all
    }
    /// Every way a draft goes: out of the map, its send token cancelled, its upload dropped — in one step.
    fn take_out(&mut self, room: &str) -> Option<Draft> {
        let d = self.drafts.remove(room)?;
        d.token.cancel();
        if !d.txn.is_empty() {
            self.uploads.remove(&d.txn);
        }
        Some(d)
    }
    /// Cache a finished upload for `txn` — only while that draft may still send (else nothing is kept: false).
    pub fn put_upload(&mut self, account: &str, txn: &str, media: tinhat_widgets::voicenote::VoiceMedia, size: u64) -> bool {
        if !self.allows_send(account, txn) {
            return false;
        }
        self.uploads.insert(txn.to_string(), (media, size));
        true
    }
    /// The draft's send token, while `txn` may still send.
    pub fn token_of(&self, account: &str, txn: &str) -> Option<SendToken> {
        if !self.allows_send(account, txn) {
            return None;
        }
        self.drafts.values().find(|d| d.txn == txn).map(|d| d.token.clone())
    }
    pub fn recording(&self) -> Option<&Draft> {
        self.drafts.values().find(|d| d.state.capturing())
    }
    /// Start a draft in `room` (requesting). Refused while another recording runs or the room already has a draft.
    pub fn begin(&mut self, room: &str, owner: u64, path: impl FnOnce(u64) -> String) -> Result<u64, &'static str> {
        if room.is_empty() {
            return Err("no room");
        }
        if self.recording().is_some() {
            return Err("busy");
        }
        if self.drafts.contains_key(room) {
            return Err("draft");
        }
        self.next += 1;
        let id = self.next;
        self.drafts.insert(
            room.to_string(),
            Draft {
                id,
                room: room.to_string(),
                owner,
                state: VnState::Requesting,
                path: path(id),
                samples: 0,
                duration_ms: 0,
                waveform: vec![],
                size: 0,
                txn: String::new(),
                error: String::new(),
                send_after_stop: false,
                req_pending: false,
                token: SendToken::default(),
            },
        );
        Ok(id)
    }
    fn by_id(&mut self, id: u64) -> Option<&mut Draft> {
        self.drafts.values_mut().find(|d| d.id == id)
    }
    /// The recorder answered: running. false = stale (cancelled meanwhile: the caller cancels the recorder).
    pub fn started(&mut self, id: u64) -> bool {
        match self.by_id(id) {
            Some(d) if d.state == VnState::Requesting => {
                d.state = VnState::Recording;
                true
            }
            _ => false,
        }
    }
    /// The recorder refused / failed: the draft goes (returned when it was current).
    pub fn start_failed(&mut self, id: u64) -> Option<Draft> {
        let room = self.drafts.values().find(|d| d.id == id && d.state == VnState::Requesting)?.room.clone();
        self.take_out(&room)
    }
    /// Stop the room's recording: its id (None = nothing recording there).
    pub fn stop(&mut self, room: &str) -> Option<u64> {
        let d = self.drafts.get_mut(room)?;
        if d.state != VnState::Recording {
            return None;
        }
        d.state = VnState::Finalizing;
        Some(d.id)
    }
    /// The recording `id` finished (EOS reached, or stopped by a call / the cap / a device failure).
    pub fn finalized(&mut self, id: u64, f: &tinhat_streamer::voicenote::Finished) -> Finalized {
        let path = f.path.to_string_lossy().to_string();
        let Some(d) = self.by_id(id) else { return Finalized::Stale(path) };
        // (requesting too: a call can stop a recording whose start answer has not been folded in yet)
        if !d.state.capturing() || d.path != path {
            return Finalized::Stale(path);
        }
        let room = d.room.clone();
        // review r1 #4: only a cleanly finalized file, or one an independent decode salvaged, can be reviewed / sent
        if !f.sendable() {
            self.take_out(&room);
            return Finalized::Failed(path);
        }
        if f.duration_ms < VOICE_MIN_MS {
            self.take_out(&room);
            return Finalized::TooShort(path);
        }
        d.samples = f.samples;
        d.duration_ms = f.duration_ms;
        d.waveform = f.waveform.clone();
        d.size = f.size;
        d.state = VnState::Review;
        d.error = if f.error.is_some() || f.reason == tinhat_streamer::voicenote::Reason::Error {
            "The microphone stopped".into()
        } else {
            String::new()
        };
        if std::mem::take(&mut d.send_after_stop) {
            return Finalized::Send;
        }
        Finalized::Review
    }
    /// Send the room's note: review → sending (minting the txn once); recording → stop first, send when finalized.
    /// Returns the id of a recording to stop (the caller stops the recorder), else None.
    pub fn send(&mut self, room: &str, mint: impl FnOnce() -> String) -> Option<u64> {
        let d = self.drafts.get_mut(room)?;
        match d.state {
            VnState::Recording => {
                d.send_after_stop = true;
                d.state = VnState::Finalizing;
                Some(d.id)
            }
            VnState::Finalizing => {
                d.send_after_stop = true;
                None
            }
            VnState::Review => {
                if d.txn.is_empty() {
                    d.txn = mint();
                }
                d.error.clear();
                d.state = VnState::Sending;
                d.req_pending = true;
                None
            }
            _ => None,
        }
    }
    /// The view collects the room's send request (once).
    pub fn take_request(&mut self, room: &str) -> Option<Draft> {
        let d = self.drafts.get_mut(room)?;
        if d.state != VnState::Sending || !d.req_pending {
            return None;
        }
        d.req_pending = false;
        Some(d.clone())
    }
    /// A send of `txn` ended. Ok → the draft is done (returned: delete its file); Err → back to review with the error.
    /// A result for a txn no draft carries (deleted meanwhile) is dropped.
    pub fn send_result(&mut self, txn: &str, r: &Result<String, String>) -> Option<Draft> {
        if txn.is_empty() {
            return None;
        }
        let room = self.drafts.values().find(|d| d.txn == txn)?.room.clone();
        match r {
            Ok(_) => self.take_out(&room),
            Err(e) => {
                let d = self.drafts.get_mut(&room)?;
                if matches!(d.state, VnState::Sending | VnState::Review) {
                    d.state = VnState::Review;
                    d.req_pending = false;
                    d.error = if e == "Couldn't send" { "Couldn't send — try again".into() } else { e.clone() };
                }
                None
            }
        }
    }
    /// Cancel the room's recording (requesting / recording / finalizing): the draft goes (returned).
    pub fn cancel(&mut self, room: &str) -> Option<Draft> {
        if !self.drafts.get(room)?.state.capturing() {
            return None;
        }
        self.take_out(room)
    }
    /// Delete the room's reviewed note (not while it is being sent).
    pub fn delete(&mut self, room: &str) -> Option<Draft> {
        if self.drafts.get(room)?.state != VnState::Review {
            return None;
        }
        self.take_out(room)
    }
    /// The room was left: its draft goes, whatever its state.
    pub fn discard_room(&mut self, room: &str) -> Option<Draft> {
        self.take_out(room)
    }
    /// The view `owner` now shows `room` ("" = it closed): a recording it started in another room stops into review;
    /// one still opening the microphone is cancelled (review r1 #6: a late start never records in an abandoned room).
    pub fn view_moved(&mut self, owner: u64, room: &str) -> Option<Moved> {
        // a note already finalizing for a Send pressed in that room is not sent from a room nobody looks at: review
        for d in self.drafts.values_mut().filter(|d| d.owner == owner && d.state == VnState::Finalizing && d.room != room) {
            d.send_after_stop = false;
        }
        let r = self.drafts.values().find(|d| d.owner == owner && d.room != room && matches!(d.state, VnState::Recording | VnState::Requesting))?.room.clone();
        let d = self.drafts.get_mut(&r)?;
        if d.state == VnState::Requesting {
            let id = d.id;
            self.take_out(&r);
            return Some(Moved::Cancel(id));
        }
        d.state = VnState::Finalizing;
        d.send_after_stop = false;
        Some(Moved::Stop(d.id))
    }
    /// review r1 #5: may the send of `txn` go on (upload, cache, send_raw)? Only while a draft of this account still
    /// carries it (in review or sending): a discard (leave, sign-out, Delete, another account) stops it at its next step.
    pub fn allows_send(&self, account: &str, txn: &str) -> bool {
        !txn.is_empty()
            && self.account == account
            && self.drafts.values().any(|d| d.txn == txn && matches!(d.state, VnState::Sending | VnState::Review))
    }
}

// ── the process-wide instance ────────────────────────────────────────────────────────────────────────────────────

fn machine() -> &'static Mutex<Machine> {
    static M: OnceLock<Mutex<Machine>> = OnceLock::new();
    M.get_or_init(Default::default)
}
fn lock() -> std::sync::MutexGuard<'static, Machine> {
    machine().lock().unwrap_or_else(|p| p.into_inner())
}

/// One-shot messages for the room's view (toasts and screen-reader announcements), consumed by the first view polling
/// that room: (room, kind, text, eventId).
fn events() -> &'static Mutex<Vec<(String, &'static str, String, String)>> {
    static E: OnceLock<Mutex<Vec<(String, &'static str, String, String)>>> = OnceLock::new();
    E.get_or_init(Default::default)
}
fn emit(room: &str, kind: &'static str, text: &str) {
    emit_full(room, kind, text, "");
}
fn emit_full(room: &str, kind: &'static str, text: &str, event_id: &str) {
    events().lock().unwrap_or_else(|p| p.into_inner()).push((room.to_string(), kind, text.to_string(), event_id.to_string()));
}

fn current_account() -> String {
    crate::session::client().and_then(|c| c.user_id().map(|u| u.to_string())).unwrap_or_default()
}
/// Every entry point first: another account signed in → the old account's drafts are gone.
fn sync_account() {
    let gone = lock().set_account(&current_account());
    drop_drafts(gone);
}
fn drop_drafts(ds: Vec<Draft>) {
    for d in ds {
        if d.state.capturing() {
            tinhat_streamer::voicenote::request_cancel(d.id);
        }
        delete_recording(&d.path);
    }
}

/// A recording leaves the disk now. The first try is inline; when something still has the file open (on Windows the review
/// player or a virus scanner, a sharing violation) a short-lived thread keeps trying for about 5 s. Never silent: if it is
/// still there after that, the log says so and the next start's sweep removes it.
fn delete_recording(path: &str) {
    match std::fs::remove_file(path) {
        Ok(()) => return,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => eprintln!("[voicenote] the recording is still in use ({e}); retrying"),
    }
    let path = std::path::PathBuf::from(path);
    // Builder, not thread::spawn: a cleanup must never be able to panic (spawn panics when the OS refuses a thread)
    let worker = std::thread::Builder::new().name("voice-delete".into()).spawn(move || {
        let wait = std::time::Duration::from_millis(100);
        if !tinhat_streamer::voicenote::remove_retrying(&path, 50, wait, &mut |p| std::fs::remove_file(p)) {
            eprintln!("[voicenote] a recording could not be deleted (the next start's sweep removes it)");
        }
    });
    if worker.is_err() {
        eprintln!("[voicenote] no thread for the retry (the next start's sweep removes the recording)");
    }
}

/// room.rs: the finished upload of `txn`, if one is cached.
pub(crate) fn upload_get(txn: &str) -> Option<(tinhat_widgets::voicenote::VoiceMedia, u64)> {
    lock().uploads.get(txn).cloned()
}
/// room.rs: cache the finished upload — atomically with the draft's validity (false = the draft is gone: not kept).
pub(crate) fn upload_put_if_alive(txn: &str, media: tinhat_widgets::voicenote::VoiceMedia, size: u64) -> bool {
    let acct = current_account();
    lock().put_upload(&acct, txn, media, size)
}
pub(crate) fn upload_forget(txn: &str) {
    lock().uploads.remove(txn);
}
/// room.rs: the send token of `txn`'s draft (None = discarded already).
pub(crate) fn send_token(txn: &str) -> Option<SendToken> {
    let acct = current_account();
    lock().token_of(&acct, txn)
}

/// Sign-out (signout.rs): every draft and recording goes.
pub fn forget_session() {
    let gone = lock().clear();
    drop_drafts(gone);
}
/// The room was left (bridge.rs leave_room): its draft goes.
pub fn discard_room(room: &str) {
    let gone = lock().discard_room(room);
    drop_drafts(gone.into_iter().collect());
}

/// room.rs (start_echo): the send of `txn` ended (Ok = the event id; Err = the friendly error).
pub fn report_send(txn: &str, r: Result<String, String>) {
    let room = lock().drafts.values().find(|d| d.txn == txn).map(|d| d.room.clone());
    let done = lock().send_result(txn, &r);
    let Some(room) = room else { return };
    match (&r, done) {
        (Ok(eid), Some(d)) => {
            eprintln!("[voicenote] sent {eid} ({} ms, {} bytes)", d.duration_ms, d.size);
            emit_full(&room, "sent", "Voice message sent", eid);
            // privacy: the recording goes AT ONCE; the echo row stops pointing at it (room.rs) and the server's own row
            // (the uploaded copy) replaces it on the next sync
            delete_recording(&d.path);
        }
        (Err(e), _) => {
            eprintln!("[voicenote] send failed: {e}");
            emit(&room, "error", e);
        }
        _ => {}
    }
}

/// privacy (Windows QA 2026-11-07): the drafts' folder, inside the app's OWN data root (Linux / Flatpak:
/// `$XDG_DATA_HOME/TinHat`, Windows: `%LOCALAPPDATA%\TinHatApp`) — what the uninstaller's remove-data step, a Flatpak's
/// `--delete-data` and the sign-out wipe cover. Not the shared cache base (on Windows `%LOCALAPPDATA%` itself).
pub(crate) fn voice_dir() -> std::path::PathBuf {
    crate::bridge::data_dir().join("voice-drafts")
}

fn new_path(id: u64) -> String {
    let dir = voice_dir();
    let _ = std::fs::create_dir_all(&dir);
    let uniq = matrix_sdk::ruma::TransactionId::new().to_string();
    dir.join(format!("voice-{id}-{uniq}.ogg")).to_string_lossy().to_string()
}

/// A crash leaves its recordings behind: the first controller of a start deletes every `voice-*.ogg` in the media cache.
fn startup_sweep() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        // (the media cache too: where earlier builds of this branch kept them)
        let mut n = 0;
        for dir in [voice_dir(), crate::room::media_cache_dir()] {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if is_voice_file(&name) && std::fs::remove_file(e.path()).is_ok() {
                    n += 1;
                }
            }
        }
        if n > 0 {
            eprintln!("[voicenote] removed {n} stale recording(s) from the media cache");
        }
    });
}
pub(crate) fn is_voice_file(name: &str) -> bool {
    name.starts_with("voice-") && name.ends_with(".ogg")
}

/// Ask the recorder's own worker to stop recording `id` (non-blocking): the note comes back to [`apply_finished`]
/// through the recorder's listener, whatever views exist.
fn spawn_stop(id: u64) {
    tinhat_streamer::voicenote::request_stop(id);
}
fn act(m: Option<Moved>) {
    match m {
        Some(Moved::Stop(id)) => spawn_stop(id),
        Some(Moved::Cancel(id)) => tinhat_streamer::voicenote::request_cancel(id),
        None => {}
    }
}
/// The recorder's listener (registered once): every finished recording, on the recorder's worker thread.
fn register_listener() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| tinhat_streamer::voicenote::set_listener(|f| apply_finished(f.id, f)));
}

/// review r1 #5: the send of `txn` may go on (room.rs checks it after every await, before caching and before send_raw).
pub fn send_allowed(txn: &str) -> bool {
    let acct = current_account();
    lock().allows_send(&acct, txn)
}

fn apply_finished(id: u64, f: &tinhat_streamer::voicenote::Finished) {
    let by_call = f.reason == tinhat_streamer::voicenote::Reason::Call;
    let room = lock().drafts.values().find(|d| d.id == id).map(|d| d.room.clone()).unwrap_or_default();
    let out = lock().finalized(id, f);
    match out {
        Finalized::Review => {
            let what = if by_call {
                "Recording stopped for the call"
            } else if f.capped {
                "Recording stopped at 15 minutes"
            } else {
                "Recording stopped"
            };
            emit(&room, "announce", &format!("{what}, {}", spoken(f.duration_ms)));
            if f.error.is_some() || f.reason == tinhat_streamer::voicenote::Reason::Error {
                emit(&room, "error", "The microphone stopped");
            }
        }
        Finalized::Send => {
            let mut m = lock();
            m.send(&room, || matrix_sdk::ruma::TransactionId::new().to_string());
        }
        Finalized::TooShort(p) => {
            delete_recording(&p);
            emit(&room, "toast", "Too short to send");
        }
        Finalized::Failed(p) => {
            delete_recording(&p);
            emit(&room, "toast", "The recording failed");
        }
        Finalized::Stale(p) => {
            delete_recording(&p);
        }
    }
}

/// "7 seconds" / "1 minute 5 seconds" for the announcement.
pub fn spoken(ms: u64) -> String {
    let s = ms / 1000;
    let (m, s) = (s / 60, s % 60);
    let sec = format!("{s} second{}", if s == 1 { "" } else { "s" });
    if m == 0 {
        sec
    } else {
        format!("{m} minute{} {sec}", if m == 1 { "" } else { "s" })
    }
}

/// The live level strip: the last ~3 s of frame peaks folded to `bars` values 0..1 (square-root scale, like the
/// waveform). Decorative.
pub fn level_bars(recent: &[u32], bars: usize) -> Vec<f64> {
    let r = tinhat_streamer::voicenote::RECENT_FRAMES;
    if bars == 0 {
        return vec![];
    }
    // right-aligned: the strip is the last RECENT_FRAMES frames, silence before the recording began
    let tail = &recent[recent.len().saturating_sub(r)..];
    let pad = r - tail.len();
    let at = |i: usize| if i < pad { 0 } else { tail[i - pad] };
    (0..bars)
        .map(|b| {
            let (lo, hi) = (b * r / bars, ((b + 1) * r / bars).max(b * r / bars + 1));
            let p = (lo..hi.min(r)).map(at).max().unwrap_or(0);
            (p as f64 / 32768.0).sqrt().min(1.0)
        })
        .collect()
}

pub struct VoiceNoteControllerRust {
    probe: bool,
    auto_gate: QString,
    id: u64,
    room: String,
}

impl Default for VoiceNoteControllerRust {
    fn default() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        startup_sweep();
        register_listener();
        Self {
            probe: std::env::var("TINHAT_GUI_VOICEPROBE").map(|v| v == "1").unwrap_or(false),
            auto_gate: QString::from(std::env::var("TINHAT_GUI_VOICENOTE").unwrap_or_default().as_str()),
            id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            room: String::new(),
        }
    }
}

/// review r1 #3: a view that goes away detaches explicitly — a recording it started stops into review, a start it began
/// is cancelled — whether or not any other view is polling.
impl Drop for VoiceNoteControllerRust {
    fn drop(&mut self) {
        let m = lock().view_moved(self.id, "");
        act(m);
    }
}

fn in_call() -> bool {
    tinhat_streamer::capture_lease::session().is_some()
}

impl qobject::VoiceNoteController {
    pub fn show(mut self: Pin<&mut Self>, room_id: &QString) {
        sync_account();
        let room = room_id.to_string();
        let me = self.rust().id;
        self.as_mut().rust_mut().room = room.clone();
        let m = lock().view_moved(me, &room);
        act(m);
    }

    /// The room an action targets: "" refuses; another room than the synced one is synced first.
    fn act_room(mut self: Pin<&mut Self>, room_id: &QString) -> Option<String> {
        let room = room_id.to_string();
        if room.is_empty() {
            return None;
        }
        if self.rust().room != room {
            self.as_mut().show(room_id);
        }
        Some(room)
    }

    pub fn snapshot(self: Pin<&mut Self>) -> QString {
        sync_account();
        let room = self.rust().room.clone();
        let mut evs = vec![];
        {
            let mut e = events().lock().unwrap_or_else(|p| p.into_inner());
            e.retain(|(r, kind, text, eid)| {
                if *r == room {
                    evs.push(json!({ "kind": kind, "text": text, "eventId": eid }));
                    false
                } else {
                    true
                }
            });
        }
        let d = lock().drafts.get(&room).cloned();
        let busy_elsewhere = lock().recording().is_some_and(|r| r.room != room);
        let mut v = json!({
            "state": "idle", "inCall": in_call(), "busyElsewhere": busy_elsewhere, "events": evs,
            "elapsedMs": 0, "clock": "0:00", "warn": false, "level": 0.0, "levels": [],
            "waveform": [], "durationMs": 0, "durationClock": "0:00", "fileUrl": "", "error": "", "txn": "",
        });
        if let Some(d) = d {
            v["state"] = json!(d.state.as_str());
            v["error"] = json!(d.error);
            v["txn"] = json!(d.txn);
            if matches!(d.state, VnState::Recording | VnState::Finalizing | VnState::Requesting) {
                let st = tinhat_streamer::voicenote::status().filter(|s| s.id == d.id);
                let samples = st.as_ref().map(|s| s.samples).unwrap_or(0);
                let ms = tinhat_widgets::voicenote::duration_ms(samples, tinhat_widgets::voicenote::VOICE_RATE);
                let left = VOICE_MAX_MS.saturating_sub(ms);
                v["elapsedMs"] = json!(ms);
                v["warn"] = json!(left <= VOICE_WARN_LEFT_MS);
                v["clock"] = json!(if left <= VOICE_WARN_LEFT_MS { format!("{} left", clock(left + 999)) } else { clock(ms) });
                v["level"] = json!(st.as_ref().map(|s| (s.level as f64 / 32768.0).sqrt().min(1.0)).unwrap_or(0.0));
                v["levels"] = json!(level_bars(st.as_ref().map(|s| s.recent.as_slice()).unwrap_or(&[]), 48));
            } else {
                v["waveform"] = json!(d.waveform);
                v["durationMs"] = json!(d.duration_ms);
                v["durationClock"] = json!(clock(d.duration_ms));
                v["fileUrl"] = json!(crate::platform::file_url(std::path::Path::new(&d.path)));
            }
        }
        QString::from(&v.to_string())
    }

    pub fn start(mut self: Pin<&mut Self>, room_id: &QString) {
        sync_account();
        let Some(room) = self.as_mut().act_room(room_id) else { return };
        if in_call() {
            emit(&room, "toast", "You're in a call — voice messages are off until you leave");
            return;
        }
        if tinhat_streamer::capture_barred() {
            emit(&room, "toast", "You're watching — tap Talk first");
            return;
        }
        let owner = self.rust().id;
        let id = match lock().begin(&room, owner, new_path) {
            Ok(id) => id,
            Err(why) => {
                eprintln!("[voicenote] start refused: {why}");
                return;
            }
        };
        let path = lock().drafts.get(&room).map(|d| d.path.clone()).unwrap_or_default();
        std::thread::spawn(move || {
            // No input device: nothing starts (a bare default head would wait forever on nothing)
            if tinhat_streamer::call::mic_target_node().is_none() && tinhat_streamer::call::list_audio_inputs().is_empty() {
                if lock().start_failed(id).is_some() {
                    emit(&room, "toast", "No microphone found");
                }
                return;
            }
            match tinhat_streamer::voicenote::start(std::path::Path::new(&path), id) {
                Ok(()) => {
                    if lock().started(id) {
                        emit(&room, "announce", "Recording");
                    } else {
                        // cancelled (or its view moved on) while the microphone opened: it never records
                        tinhat_streamer::voicenote::request_cancel(id);
                    }
                }
                Err(e) => {
                    use tinhat_streamer::voicenote::StartError;
                    let msg = match e {
                        StartError::InCall => "You're in a call — voice messages are off until you leave",
                        StartError::Watching => "You're watching — tap Talk first",
                        StartError::Busy | StartError::Failed(_) => "Couldn't start recording",
                        StartError::Cancelled => "",
                    };
                    eprintln!("[voicenote] start failed: {e:?}");
                    delete_recording(&path);
                    if lock().start_failed(id).is_some() && !msg.is_empty() {
                        emit(&room, "toast", msg);
                    }
                }
            }
        });
    }

    pub fn stop(mut self: Pin<&mut Self>, room_id: &QString) {
        let Some(room) = self.as_mut().act_room(room_id) else { return };
        let id = lock().stop(&room);
        if let Some(id) = id {
            spawn_stop(id);
        }
    }

    pub fn cancel(mut self: Pin<&mut Self>, room_id: &QString) {
        let Some(room) = self.as_mut().act_room(room_id) else { return };
        let gone = lock().cancel(&room);
        if let Some(d) = gone {
            // the recorder's worker NULLs it and deletes the file (a start still opening is cancelled)
            tinhat_streamer::voicenote::request_cancel(d.id);
            emit(&room, "announce", "Recording cancelled");
        }
    }

    pub fn send(mut self: Pin<&mut Self>, room_id: &QString) {
        let Some(room) = self.as_mut().act_room(room_id) else { return };
        let stop = lock().send(&room, || matrix_sdk::ruma::TransactionId::new().to_string());
        if let Some(id) = stop {
            spawn_stop(id);
        }
    }

    pub fn take_send_request(mut self: Pin<&mut Self>, room_id: &QString) -> QString {
        let Some(room) = self.as_mut().act_room(room_id) else { return QString::default() };
        let Some(d) = lock().take_request(&room) else { return QString::default() };
        QString::from(
            &json!({
                "room": d.room, "path": d.path, "durationMs": d.duration_ms, "waveform": d.waveform, "size": d.size, "txn": d.txn,
            })
            .to_string(),
        )
    }

    pub fn delete_draft(mut self: Pin<&mut Self>, room_id: &QString) -> QString {
        let Some(room) = self.as_mut().act_room(room_id) else { return QString::default() };
        let gone = lock().delete(&room);
        match gone {
            Some(d) => {
                let txn = d.txn.clone();
                drop_drafts(vec![d]);
                emit(&room, "announce", "Voice message deleted");
                QString::from(&txn)
            }
            None => QString::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tinhat_streamer::voicenote::Finished;

    fn fin(id: u64, path: &str, ms: u64) -> Finished {
        Finished {
            id,
            path: path.into(),
            samples: ms * 48,
            duration_ms: ms,
            waveform: vec![1, 2, 3],
            size: 1000,
            reason: tinhat_streamer::voicenote::Reason::Stop,
            capped: false,
            error: None,
            clean: true,
            salvaged: false,
        }
    }

    fn rec(m: &mut Machine, room: &str) -> (u64, String) {
        let id = m.begin(room, 1, |id| format!("/c/voice-{id}.ogg")).unwrap();
        assert!(m.started(id));
        (id, format!("/c/voice-{id}.ogg"))
    }

    #[test]
    fn the_happy_path_walks_every_state() {
        let mut m = Machine::default();
        let id = m.begin("!a", 1, |id| format!("/c/voice-{id}.ogg")).unwrap();
        assert_eq!(m.drafts["!a"].state, VnState::Requesting);
        assert!(m.started(id));
        assert_eq!(m.drafts["!a"].state, VnState::Recording);
        assert_eq!(m.stop("!a"), Some(id));
        assert_eq!(m.drafts["!a"].state, VnState::Finalizing);
        assert_eq!(m.finalized(id, &fin(id, "/c/voice-1.ogg", 7000)), Finalized::Review);
        assert_eq!(m.drafts["!a"].state, VnState::Review);
        assert_eq!(m.drafts["!a"].duration_ms, 7000);
        assert_eq!(m.send("!a", || "T1".into()), None);
        assert_eq!(m.drafts["!a"].state, VnState::Sending);
        let req = m.take_request("!a").unwrap();
        assert_eq!(req.txn, "T1");
        assert!(m.take_request("!a").is_none(), "a request is handed out once");
        let done = m.send_result("T1", &Ok("$e".into())).unwrap();
        assert_eq!(done.path, "/c/voice-1.ogg");
        assert!(m.drafts.is_empty());
    }

    #[test]
    fn a_retry_keeps_the_txn_and_the_draft() {
        let mut m = Machine::default();
        let (id, p) = rec(&mut m, "!a");
        m.stop("!a");
        m.finalized(id, &fin(id, &p, 3000));
        let mut minted = 0;
        m.send("!a", || {
            minted += 1;
            "T1".into()
        });
        m.take_request("!a").unwrap();
        assert!(m.send_result("T1", &Err("Couldn't send".into())).is_none());
        let d = &m.drafts["!a"];
        assert_eq!(d.state, VnState::Review, "a failed send keeps the draft in review");
        assert_eq!(d.error, "Couldn't send — try again");
        m.send("!a", || {
            minted += 1;
            "T2".into()
        });
        assert_eq!(minted, 1, "the txn is minted once");
        assert_eq!(m.take_request("!a").unwrap().txn, "T1", "the retry uses the same txn");
        assert!(m.drafts["!a"].error.is_empty());
        assert!(m.send_result("T1", &Ok("$e".into())).is_some());
    }

    #[test]
    fn the_upload_is_reused_by_txn() {
        use tinhat_widgets::voicenote::VoiceMedia;
        let mut m = Machine::default();
        m.account = "@a:x".into();
        let (id, p) = rec(&mut m, "!a");
        m.stop("!a");
        m.finalized(id, &fin(id, &p, 2000));
        m.send("!a", || "TX-reuse".into());
        assert!(m.put_upload("@a:x", "TX-reuse", VoiceMedia::Url("mxc://s/one".into()), 42));
        match m.uploads.get("TX-reuse") {
            Some((VoiceMedia::Url(u), 42)) => assert_eq!(u, "mxc://s/one"),
            other => panic!("{other:?}"),
        }
        m.send_result("TX-reuse", &Err("Couldn't send".into()));
        assert!(m.uploads.contains_key("TX-reuse"), "a failed send keeps the finished upload for the retry");
        m.send_result("TX-reuse", &Ok("$e".into()));
        assert!(m.uploads.is_empty(), "sent: the upload goes with the draft");
        assert!(!crate::room::voice_over_limit(100, None), "unknown limit: no refusal");
        assert!(!crate::room::voice_over_limit(100, Some(0)), "a zero limit is unknown, never zero");
        assert!(!crate::room::voice_over_limit(100, Some(100)));
        assert!(crate::room::voice_over_limit(101, Some(100)));
        // review r1 #1: a cached plaintext upload is never sent into a room that turned encrypted
        let url = VoiceMedia::Url("mxc://s/one".into());
        let file = VoiceMedia::File(serde_json::json!({"v": "v2", "url": "mxc://s/two"}));
        assert!(crate::room::voice_upload_fits(&url, false));
        assert!(!crate::room::voice_upload_fits(&url, true), "plaintext media into an encrypted room: re-upload");
        // security review round 4 (VN-1 parity / VN-2): encrypted if EITHER read says so; Unknown uploads nothing
        {
            use matrix_sdk::EncryptionState as E;
            let enc = crate::room::voice_upload_encrypt;
            assert_eq!(enc(&E::NotEncrypted, &E::NotEncrypted), Ok(false));
            assert_eq!(enc(&E::Encrypted, &E::Encrypted), Ok(true));
            assert_eq!(enc(&E::NotEncrypted, &E::Encrypted), Ok(true), "turned encrypted between the send-time read and the upload (VN-2)");
            assert_eq!(enc(&E::Encrypted, &E::NotEncrypted), Ok(true), "never downgraded by the later read");
            assert!(enc(&E::Unknown, &E::NotEncrypted).is_err(), "can't tell at send time: nothing uploaded");
            assert!(enc(&E::NotEncrypted, &E::Unknown).is_err(), "can't tell at the upload: nothing uploaded");
            assert!(enc(&E::Unknown, &E::Encrypted).is_err());
            // the send path uses it: once at send time, again right before the upload (after the read and the limit
            // lookup awaited, nothing awaited between the re-read and the request), and after the upload
            let src = include_str!("room.rs");
            let at = src.find("EchoKind::Voice(v) => {").expect("the voice send path");
            let body = &src[at..at + 9000.min(src.len() - at)];
            let first = body.find("let encrypted = voice_upload_encrypt(&at_send, &at_send)?;").expect("send-time read");
            let limit = body.find("load_or_fetch_max_upload_size().await").expect("limit lookup");
            let reread = body.find("let encrypted = voice_upload_encrypt(&at_send, &room.encryption_state())?;").expect("the re-read before the upload");
            let choose = body.find("let media = if encrypted {").expect("the upload choice");
            assert!(first < limit && limit < reread && reread < choose, "re-read after the awaits, right before the choice");
            assert!(!body[reread..choose].contains(".await"), "nothing awaited between the re-read and the upload choice");
            assert!(body.contains("let now = voice_upload_encrypt(&now_state, &room.encryption_state())?;"), "the post-upload check too");
            assert!(!body.contains(".is_encrypted();"), "no bare is_encrypted() decision left in the voice path");
        }
        assert!(crate::room::voice_upload_fits(&file, true) && crate::room::voice_upload_fits(&file, false));
    }

    #[test]
    fn stale_answers_are_dropped() {
        let mut m = Machine::default();
        // start answered after a cancel
        let id = m.begin("!a", 1, |id| format!("/c/voice-{id}.ogg")).unwrap();
        assert!(m.cancel("!a").is_some());
        assert!(!m.started(id), "a late start answer after Cancel is dropped");
        // EOS of a cancelled recording
        let (id2, p2) = rec(&mut m, "!a");
        m.cancel("!a");
        assert_eq!(m.finalized(id2, &fin(id2, &p2, 3000)), Finalized::Stale(p2.clone()));
        // a new recording in the same room never takes an old id's EOS
        let (id3, p3) = rec(&mut m, "!a");
        m.stop("!a");
        assert_eq!(m.finalized(id2, &fin(id2, &p2, 3000)), Finalized::Stale(p2));
        assert_eq!(m.finalized(id3, &fin(id3, &p3, 3000)), Finalized::Review);
        // a send result for a deleted draft
        m.send("!a", || "T9".into());
        m.take_request("!a");
        m.send_result("T9", &Err("x".into()));
        assert!(m.delete("!a").is_some());
        assert!(m.send_result("T9", &Ok("$late".into())).is_none());
        // another account: every draft goes
        let _ = rec(&mut m, "!b");
        m.account = "@a:x".into();
        assert_eq!(m.set_account("@a:x").len(), 0);
        assert_eq!(m.set_account("@b:x").len(), 1);
        assert!(m.drafts.is_empty());
    }

    #[test]
    fn one_recording_at_a_time_and_room_switches_stop_into_review() {
        let mut m = Machine::default();
        let (id, p) = rec(&mut m, "!a");
        assert_eq!(m.begin("!b", 1, |_| "x".into()), Err("busy"));
        // the same view moves to !b: !a's recording stops (into review, not sent)
        m.send("!a", || "T".into()); // Send pressed while recording …
        assert_eq!(m.drafts["!a"].state, VnState::Finalizing);
        let mut m2 = Machine::default();
        let (id2, p2) = rec(&mut m2, "!a");
        assert_eq!(m2.view_moved(1, "!b"), Some(Moved::Stop(id2)));
        m2.drafts.get_mut("!a").unwrap().send_after_stop = true; // (a Send pressed just before the switch)
        assert_eq!(m2.view_moved(1, "!b"), None);
        assert!(!m2.drafts["!a"].send_after_stop, "moving away never sends from a room nobody looks at");
        assert_eq!(m2.view_moved(1, "!b"), None);
        assert_eq!(m2.finalized(id2, &fin(id2, &p2, 2000)), Finalized::Review);
        // another view of another room does not stop it
        let (id3, _) = rec(&mut m2, "!c");
        assert_eq!(m2.view_moved(2, "!d"), None);
        assert_eq!(m2.drafts["!c"].state, VnState::Recording);
        let _ = id3;
        // Send while recording finalizes straight into sending
        assert_eq!(m.finalized(id, &fin(id, &p, 2000)), Finalized::Send);
        m.send("!a", || "T".into());
        assert_eq!(m.drafts["!a"].state, VnState::Sending);
        // a room with a draft waits: no second draft there
        assert_eq!(m.begin("!a", 1, |_| "x".into()), Err("draft"));
    }

    #[test]
    fn too_short_and_empty_notes_are_discarded() {
        let mut m = Machine::default();
        let (id, p) = rec(&mut m, "!a");
        m.stop("!a");
        assert_eq!(m.finalized(id, &fin(id, &p, 499)), Finalized::TooShort(p));
        assert!(m.drafts.is_empty());
        let (id, p) = rec(&mut m, "!a");
        let mut f = fin(id, &p, 3000);
        f.size = 0;
        assert_eq!(m.finalized(id, &f), Finalized::Failed(p));
        // a device failure with something captured: review + the note
        let (id, p) = rec(&mut m, "!a");
        let mut f = fin(id, &p, 3000);
        f.error = Some("device gone".into());
        f.clean = false;
        f.salvaged = true; // an independent decode proved it
        assert_eq!(m.finalized(id, &f), Finalized::Review);
        assert_eq!(m.drafts["!a"].error, "The microphone stopped");
    }

    /// review r1 #4: a file that was not cleanly finalized and not salvaged by a decode never becomes sendable
    #[test]
    fn an_unfinalized_file_is_never_sendable() {
        let mut m = Machine::default();
        let (id, p) = rec(&mut m, "!a");
        m.stop("!a");
        let mut f = fin(id, &p, 5000);
        f.clean = false; // EOS timeout / encoder or disk error, no decode
        f.error = Some("write error".into());
        assert_eq!(m.finalized(id, &f), Finalized::Failed(p));
        assert!(m.drafts.is_empty(), "nothing to review, nothing to send");
    }

    /// review r1 #6: a room switch while the microphone is still opening cancels that start; its late answer is dropped
    #[test]
    fn a_room_switch_during_the_start_cancels_it() {
        let mut m = Machine::default();
        let id = m.begin("!a", 1, |id| format!("/c/voice-{id}.ogg")).unwrap();
        assert_eq!(m.view_moved(1, "!b"), Some(Moved::Cancel(id)));
        assert!(m.drafts.is_empty());
        assert!(!m.started(id), "the late start never enters Recording in the abandoned room");
        // a closing view (room "") detaches the same way
        let id = m.begin("!a", 3, |id| format!("/c/voice-{id}.ogg")).unwrap();
        assert!(m.started(id));
        assert_eq!(m.view_moved(3, ""), Some(Moved::Stop(id)));
        assert_eq!(m.drafts["!a"].state, VnState::Finalizing);
    }

    /// review r2 #4: the cache insert and the discard share one lock — once discarded, an upload that finishes later is
    /// not kept, and the draft's send token is cancelled (send_raw races it); a retry keeps txn and token
    #[test]
    fn discard_and_cache_are_atomic_and_cancel_the_token() {
        use tinhat_widgets::voicenote::VoiceMedia;
        let mut m = Machine::default();
        m.account = "@a:x".into();
        let (id, p) = rec(&mut m, "!a");
        m.stop("!a");
        m.finalized(id, &fin(id, &p, 2000));
        m.send("!a", || "T1".into());
        let tok = m.token_of("@a:x", "T1").unwrap();
        assert!(m.put_upload("@a:x", "T1", VoiceMedia::Url("mxc://s/1".into()), 1));
        m.send_result("T1", &Err("Couldn't send".into()));
        m.send("!a", || "T2".into());
        assert_eq!(m.drafts["!a"].txn, "T1", "the retry keeps the txn");
        assert!(m.token_of("@a:x", "T1").unwrap() == tok && !tok.is_cancelled(), "and the token");
        m.discard_room("!a");
        assert!(tok.is_cancelled(), "discard cancels the in-flight send");
        assert!(m.uploads.is_empty(), "and drops its upload in the same step");
        assert!(!m.put_upload("@a:x", "T1", VoiceMedia::Url("mxc://s/late".into()), 1), "a late upload is not re-inserted");
        assert!(m.uploads.is_empty());
        assert!(m.token_of("@a:x", "T1").is_none());
        // sign-out / another account cancel every token
        let (id, p) = rec(&mut m, "!b");
        m.stop("!b");
        m.finalized(id, &fin(id, &p, 2000));
        m.send("!b", || "T3".into());
        let t3 = m.token_of("@a:x", "T3").unwrap();
        m.set_account("@b:x");
        assert!(t3.is_cancelled());
        // a cancelled token wakes a waiting send at once (what send_raw races)
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let t = SendToken::default();
        let t2 = t.clone();
        let r = rt.block_on(async move {
            let wait = t2.cancelled();
            let cancel = async {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                t.cancel();
                std::future::pending::<()>().await
            };
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                tokio::select! { _ = wait => true, _ = cancel => false }
            })
            .await
        });
        assert_eq!(r, Ok(true));
    }

    /// review r1 #5: a discarded draft (leave, sign-out, Delete, another account) stops its in-flight send
    #[test]
    fn a_discard_invalidates_the_send() {
        let mut m = Machine::default();
        m.account = "@a:x".into();
        let (id, p) = rec(&mut m, "!a");
        m.stop("!a");
        m.finalized(id, &fin(id, &p, 2000));
        m.send("!a", || "T1".into());
        assert!(m.allows_send("@a:x", "T1"));
        assert!(!m.allows_send("@b:x", "T1"), "another account's send never goes on");
        assert!(!m.allows_send("@a:x", "T2"));
        assert!(!m.allows_send("@a:x", ""));
        m.send_result("T1", &Err("Couldn't send".into()));
        assert!(m.allows_send("@a:x", "T1"), "a legitimate retry keeps its txn");
        m.discard_room("!a");
        assert!(!m.allows_send("@a:x", "T1"), "left the room: the upload / send stops at its next step");
        let (id, p) = rec(&mut m, "!b");
        m.stop("!b");
        m.finalized(id, &fin(id, &p, 2000));
        m.send("!b", || "T3".into());
        m.clear();
        assert!(!m.allows_send("@a:x", "T3"), "signed out");
    }

    #[test]
    fn delete_only_in_review_and_leave_drops_anything() {
        let mut m = Machine::default();
        let (id, p) = rec(&mut m, "!a");
        assert!(m.delete("!a").is_none(), "Delete is a review action");
        m.stop("!a");
        m.finalized(id, &fin(id, &p, 1000));
        m.send("!a", || "T".into());
        assert!(m.delete("!a").is_none(), "not while sending");
        assert!(m.discard_room("!a").is_some());
    }

    /// privacy (Windows QA 2026-11-07): a recording lives inside the app's OWN data root — the folder the uninstaller's
    /// remove-data step, the Flatpak's --delete-data and the sign-out wipe cover — never in the shared cache base
    #[test]
    fn recordings_live_inside_the_data_root() {
        let p = std::path::PathBuf::from(new_path(1));
        let root = crate::bridge::data_dir();
        assert!(p.starts_with(&root), "{} is not under the data root {}", p.display(), root.display());
        let _ = std::fs::remove_dir(p.parent().unwrap());
    }

    /// privacy: the Windows uninstaller (and its maintenance script) always remove the voice drafts' folder — with or
    /// without /REMOVEDATA (drafts are in memory only: a file left over is never wanted), and the remove-data lists name it
    #[test]
    fn the_uninstaller_removes_the_voice_drafts() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../windows");
        let iss = std::fs::read_to_string(root.join("installer.iss")).unwrap();
        let ps = std::fs::read_to_string(root.join("tinhat-maint.ps1")).unwrap();
        let body = |src: &str, start: &str, end: &str| -> String {
            let i = src.find(start).unwrap_or_else(|| panic!("no {start}"));
            src[i..][..src[i..].find(end).unwrap_or(src.len() - i)].to_string()
        };
        let footprint = body(&iss, "procedure RemoveFootprint(", "\nend;\n");
        let unconditional = &footprint[..footprint.find("if not KeepSignIn").expect("the keep-sign-in branch")];
        assert!(unconditional.contains("DeleteTree(R + '\\voice-drafts')"), "uninstall (also keeping the sign-in) removes the voice drafts");
        assert!(body(&iss, "procedure DeleteKnownRootData(", "\nend;\n").contains("|voice-drafts|"), "the remove-data list names it");
        assert!(body(&iss, "procedure RepairFootprint(", "\nend;\n").contains("voice-drafts"), "a repair drops stale drafts too");
        assert!(body(&ps, "$KnownDataNames = @(", ")").contains("'voice-drafts'"), "the maintenance script's list names it");
        assert!(body(&ps, "function Remove-Sandboxed(", "foreach ($n in $KnownDataNames)").contains("'voice-drafts'"), "a sandboxed copy's drafts always go");
    }

    /// privacy: a sent recording's file is deleted AT ONCE (not later); cancel / delete / sign-out delete theirs too
    #[test]
    fn a_recording_file_never_outlives_its_draft() {
        let dir = std::env::temp_dir().join(format!("vn-files-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mk = |n: &str| {
            let p = dir.join(n);
            std::fs::write(&p, b"OggS").unwrap();
            p.to_string_lossy().to_string()
        };
        // sent
        let sent = mk("voice-sent.ogg");
        {
            let mut m = lock();
            let id = m.begin("!files-a", 77, |_| sent.clone()).unwrap();
            m.started(id);
            m.stop("!files-a");
            let mut f = tests_fin(id, &sent, 2000);
            f.path = sent.clone().into();
            m.finalized(id, &f);
            m.send("!files-a", || "TX-files".into());
        }
        report_send("TX-files", Ok("$sent".into()));
        assert!(!std::path::Path::new(&sent).exists(), "the recording of a sent note is gone at once");
        // signed out with a draft in review
        let kept = mk("voice-review.ogg");
        {
            let mut m = lock();
            let id = m.begin("!files-b", 77, |_| kept.clone()).unwrap();
            m.started(id);
            m.stop("!files-b");
            m.finalized(id, &tests_fin(id, &kept, 2000));
        }
        forget_session();
        assert!(!std::path::Path::new(&kept).exists(), "sign-out deletes the drafts' recordings");
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn every_recording_delete_goes_through_the_retrying_path() {
        let src = include_str!("voicenote_ctl.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        let direct = "let _ = std::fs::".to_string() + "remove_file";
        assert!(!prod.contains(&direct), "a recording delete that can fail silently");
    }

    fn tests_fin(id: u64, path: &str, ms: u64) -> Finished {
        fin(id, path, ms)
    }

    #[test]
    fn helpers() {
        assert_eq!(spoken(7000), "7 seconds");
        assert_eq!(spoken(1000), "1 second");
        assert_eq!(spoken(65_000), "1 minute 5 seconds");
        assert!(is_voice_file("voice-3-abc.ogg"));
        assert!(!is_voice_file("voice-3.png") && !is_voice_file("image.ogg"));
        let bars = level_bars(&[32768; 300], 48);
        assert_eq!(bars.len(), 48);
        assert!(bars.iter().all(|&b| (b - 1.0).abs() < 1e-9));
        let bars = level_bars(&[], 48);
        assert!(bars.iter().all(|&b| b == 0.0));
        // a young recording scrolls in from the right
        let bars = level_bars(&[32768; 30], 48);
        assert_eq!(bars.len(), 48);
        assert!(bars[47] > 0.99 && bars[0] == 0.0);
    }
}
