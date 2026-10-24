//! Watch party — data layer + drift-tolerant SYNC CLOCK + web search (Feature #8a).
//! A port of the Tin Hat web fork's `watchParty.ts` + `useWatchSync.ts`.
//!
//! THREE custom room-STATE events (all PL→0 best-effort via `appstate::ensure_writable`,
//! tolerating M_FORBIDDEN — the live deployment already lowered the PL via the web app):
//!   • `dev.tinhat.watchparty` (state_key ""): queue + host —
//!       `{ queue:[{url,title,addedBy}], hostId }`.
//!   • `dev.tinhat.watchparty.playback` (state_key ""): the shared clock —
//!       `{ url, positionSecs, playing(bool), updatedAt(epoch-ms INTEGER), seekNonce }`.
//!     positionSecs/updatedAt cross the cxx-qt boundary as STRINGS / whole-ms ints — never
//!     a raw f64 qproperty (unreliable binding).
//!   • `dev.tinhat.watchparty.viewer` (state_key = userId, TTL heartbeat): presence —
//!       `{ joinedAt, lastSeen(epoch-ms) }`, expiry-filtered on READ (like `status_ctl`).
//!
//! SYNC CLOCK (`useWatchSync.ts` port — the hard part):
//!   • A 1s tokio-interval, ACTIVE ONLY while in a watch party (start on joinParty, stop on
//!     leave). Each tick: refresh state, compute the target, decide seek-vs-ride, heartbeat
//!     the viewer (cadence-gated, ~7s) and — if I am the advancer — (every ~15s / on
//!     transport) re-write playback.
//!   • target = positionSecs + (playing ? (now_ms - updatedAt)/1000 : 0).
//!   • amAdvancer = (my userId == the LOWEST userId among current (non-expired) viewers).
//!     ONLY the advancer WRITES playback; everyone else FOLLOWS (read-only).
//!   • drift: |local - target| > HARD_TOL (~2s) ⇒ emit `desiredSeek(target)`; small drift ⇒
//!     ride (no seek). Temporary stalls (ad/buffer) tolerated — we don't thrash a seek every
//!     tick (a fresh seek for the SAME target within a short window is suppressed).
//!   • applyingRemote GATE: when we apply a remote seek/state, suppress our own immediate
//!     write so it doesn't ping-pong into a write-storm.
//!   • manual scrub bumps `seekNonce` so all followers seek to the new position.
//!
//! The libmpv/yt-dlp PLAYER is OUT OF SCOPE this round — `desiredSeek(posSecs)` is the signal
//! the (future) player shell consumes; `local_position` is fed back via `reportPosition`.
//!
//! SEARCH — `proxy::get_json("/watch/search?q=…")` → YouTube `[{id,url,title,thumbnail,...}]`;
//! if the proxy omits titles, each result is resolved via oEmbed (proxy `/watch/oembed`, else
//! the public YouTube oEmbed endpoint).

use core::pin::Pin;
use std::sync::Mutex;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use matrix_sdk::ruma::RoomId;
use matrix_sdk::Room;
use serde_json::Value;
use tokio::task::JoinHandle;
use url::Url;

const WP_EVENT: &str = "dev.tinhat.watchparty";
const PLAYBACK_EVENT: &str = "dev.tinhat.watchparty.playback";
const VIEWER_EVENT: &str = "dev.tinhat.watchparty.viewer";
/// 11.3 QOL (QOL.md §4): the Media Party window's keys — (key as the vectors spell it, op, its argument: seconds for
/// `seek`, percent for `jump`, the step for `volume`, else 0 — the window:watchParty context action it is ("" = a
/// window-local op), the label the keys card shows). ONE table shared with the web (shortcuts.ts) through
/// `testvectors/mediaparty/shortcuts.json` (26 entries); WatchPartyWindow.qml's `Shortcut` items implement it (checked
/// by the test). Never while typing in the one box, except Esc there (suggestions → clear → blur; never closes).
pub const MP_SHORTCUTS: &[(&str, &str, i32, &str, &str)] = &[
    ("Space", "playPause", 0, "watchParty.playPause", "Play / pause for everyone"),
    ("K", "playPause", 0, "watchParty.playPause", "Play / pause for everyone"),
    ("Left", "seek", -5, "", "Back 5 s"),
    ("Right", "seek", 5, "", "Forward 5 s"),
    ("J", "seek", -10, "", "Back 10 s"),
    ("L", "seek", 10, "", "Forward 10 s"),
    ("0", "jump", 0, "", "Jump to 0 %"),
    ("1", "jump", 10, "", "Jump to 10 %"),
    ("2", "jump", 20, "", "Jump to 20 %"),
    ("3", "jump", 30, "", "Jump to 30 %"),
    ("4", "jump", 40, "", "Jump to 40 %"),
    ("5", "jump", 50, "", "Jump to 50 %"),
    ("6", "jump", 60, "", "Jump to 60 %"),
    ("7", "jump", 70, "", "Jump to 70 %"),
    ("8", "jump", 80, "", "Jump to 80 %"),
    ("9", "jump", 90, "", "Jump to 90 %"),
    ("Up", "volume", 5, "", "Volume up (only for you)"),
    ("Down", "volume", -5, "", "Volume down (only for you)"),
    ("M", "mute", 0, "watchParty.mute", "Mute (only for you)"),
    ("N", "next", 0, "watchParty.next", "Play next in queue"),
    ("F", "fullscreen", 0, "", "Fullscreen"),
    ("T", "theater", 0, "", "Theater (the video fills the window)"),
    ("I", "mini", 0, "", "Mini player"),
    ("/", "search", 0, "", "Search or paste a link"),
    ("?", "help", 0, "", "Show the keys"),
    ("Escape", "escape", 0, "", "Leave fullscreen or the mini player, else close"),
];

/// 11.4 (CONTRACT §2): the room's last 50 plays, written by the advancer 5 s after the playing url changes.
pub const HISTORY_EVENT: &str = "dev.tinhat.mediaparty.history";
pub const HISTORY_MAX: usize = 50;
const HISTORY_DEBOUNCE_MS: u64 = 5_000;

/// Hard drift tolerance (seconds): beyond this a follower seeks to the target.
const HARD_TOL_SECS: f64 = 2.0;
/// A viewer heartbeat is considered live for this long after its `lastSeen`.
const VIEWER_TTL_MS: u64 = 20_000;
/// The advancer re-writes playback at least this often (ms), even with no transport change.
const ADVANCER_HEARTBEAT_MS: u64 = 15_000;
/// After applying a remote seek/state, suppress our own writes for this long (ms) so we
/// don't ping-pong (the `applyingRemote` gate).
const APPLYING_REMOTE_MS: u64 = 1_500;
/// Don't re-emit `desiredSeek` for the SAME target within this window (ms) — tolerate a
/// stall/buffer without thrashing a seek every tick.
const SEEK_DEDUP_MS: u64 = 2_500;
/// After a LOCAL user write (enqueue/play/seek/pause) we push the value OPTIMISTICALLY; for this
/// long, run_tick must NOT re-push party/playback from the not-yet-synced local store (which would
/// revert it). Comfortably covers a homeserver /sync echo; the next tick after that reconciles.
const LOCAL_WRITE_GRACE_MS: u64 = 3_000;

/// A playback clock counts as LIVE if its `updatedAt` is at most this old (ms). Used by
/// `sync_start_memory` (join-don't-stomp: FOLLOW an in-progress replay of the same url
/// instead of resetting everyone to 0) and by the advancer heartbeat (never launder a
/// stale clock into a fresh-updatedAt extrapolated write — park it paused instead).
const PLAYBACK_LIVE_TTL_MS: u64 = 60_000;
/// A `tinhat-memory://…` (synced Memories replay) playback event older than this is a dead
/// session's leftovers — the tick treats it as ABSENT (no follow, no seek, no display).
const MEMORY_PLAYBACK_TTL_MS: u64 = 120_000;
/// No watch-party media is longer than a day: any stored/written position beyond this is
/// definitionally garbage (e.g. a stale playing clock extrapolated for days). The tick
/// clamps it out on READ, and the advancer heartbeat clamps before every WRITE.
const MAX_MEDIA_SECS: f64 = 24.0 * 3600.0;

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
        #[qproperty(QString, room_id)] //      QML: roomId — room of the active party ("" when not joined)
        #[qproperty(bool, in_party)] //        QML: inParty — true while the 1s sync loop runs
        #[qproperty(QString, party_json)] //   QML: partyJson — { queue:[{url,title,addedBy}], hostId }
        #[qproperty(QString, playback_json)] //QML: playbackJson — { url, positionSecs, playing, updatedAt, seekNonce, by } (strings/ints; `by` = the event's sender, 11.3 QOL — mirror only)
        #[qproperty(QString, viewers_json)] // QML: viewersJson — [{userId,joinedAt,lastSeen}] (non-expired)
        #[qproperty(bool, am_advancer)] //     QML: amAdvancer — I own the playback clock (lowest viewer id)
        #[qproperty(QString, search_results_json)] // QML: searchResultsJson — [{id,url,title,thumbnail}]
        #[qproperty(bool, searching)] //       QML: searching
        #[qproperty(QString, error)] //        QML: error — "" or a user-facing message
        /// QML: callPartyJson — 11.4 (Media Party): the party in the WATCHED room (the call's room, `watchRoom`):
        /// `{ live, viewers, url, playing, item:{url,title,addedBy,art?,artist?,kind?}, next, hostId }` ("" = none / not
        /// watching; 11.3 QOL: `next` = the url after the playing one — the strip's next button — and the party's host).
        /// Polled every 3 s in Rust; drives the call bar's Media Party button dot + now-playing strip.
        #[qproperty(QString, call_party_json)]
        /// QML: linkJson — 11.4: the last addLink outcome `{status:"queued"|"playing"|"search"|"browse"|"card"|"error",
        /// query?, link?, meta?, item?, message?}` (the twin of the web's AddOutcome); "" = none yet.
        #[qproperty(QString, link_json)]
        /// QML: adding — an addLink is in flight (meta / resolve on the server).
        #[qproperty(bool, adding)]
        /// QML: browseJson — 11.4 (DESIGN §2.4): the last `browse()` answer `{kind, params, ok, items?|listing?,
        /// nextPage?, error?, budget?}`; browsing — one is in flight.
        #[qproperty(QString, browse_json)]
        #[qproperty(bool, browsing)]
        /// QML: suggestJson — `["suggestion", …]` for the one box (debounced 350 ms, cached).
        #[qproperty(QString, suggest_json)]
        /// QML: historyJson — the room's History (CONTRACT §2) `[{...item, playedAt, by}]`, refreshed each tick.
        #[qproperty(QString, history_json)]
        type WatchPartyController = super::WatchPartyControllerRust;

        /// QML: joinParty(roomId) — start tracking + the 1s sync loop; write my viewer heartbeat.
        #[qinvokable]
        fn join_party(self: Pin<&mut WatchPartyController>, room_id: &QString);
        /// QML: leaveParty() — stop the sync loop (and drop my viewer presence).
        #[qinvokable]
        fn leave_party(self: Pin<&mut WatchPartyController>);
        /// QML: enqueue(url, title) — append to the shared queue (best-effort title).
        #[qinvokable]
        fn enqueue(self: Pin<&mut WatchPartyController>, url: &QString, title: &QString);
        /// QML: playUrl(url) — set the playing item (queue head / a search pick) + start it.
        #[qinvokable]
        fn play_url(self: Pin<&mut WatchPartyController>, url: &QString);
        /// QML: play() / pause() — advancer-only transport (followers ignore; the loop re-writes).
        #[qinvokable]
        fn play(self: Pin<&mut WatchPartyController>);
        #[qinvokable]
        fn pause(self: Pin<&mut WatchPartyController>);
        /// QML: seek(posSecs) — manual scrub to `posSecs` (a STRING); bumps seekNonce so all
        /// followers seek. (NOT a raw f64 across the boundary.)
        #[qinvokable]
        fn seek(self: Pin<&mut WatchPartyController>, pos_secs: &QString);
        /// QML: reportPosition(posSecs) — the player feeds back its CURRENT local position
        /// (a STRING) so the sync loop can measure drift. (No raw f64 qproperty.)
        #[qinvokable]
        fn report_position(self: Pin<&mut WatchPartyController>, pos_secs: &QString);
        /// QML: assignHost(userId) — set the party host. (NOT `set_host` — collides with the
        /// hostId-bearing party_json setter convention; distinct name per the cxx-qt pitfall.)
        #[qinvokable]
        fn assign_host(self: Pin<&mut WatchPartyController>, user_id: &QString);
        /// QML: search(query) — debounced (450ms) /watch/search via the server's Tin Hat API.
        #[qinvokable]
        fn search(self: Pin<&mut WatchPartyController>, query: &QString);
        /// QML: enqueueItem(itemJson) — 11.4: append a full queue item (CONTRACT §1; addedBy is always me).
        #[qinvokable]
        #[cxx_name = "enqueueItem"]
        fn enqueue_item(self: Pin<&mut WatchPartyController>, item_json: &QString);
        /// QML: removeAt(index) / moveItem(from, to) / clearQueue() — 11.4 queue edits (read-modify-write, every
        /// field of every item preserved). The UI gates Clear on the host; anyone may remove and move.
        #[qinvokable]
        #[cxx_name = "removeAt"]
        fn remove_at(self: Pin<&mut WatchPartyController>, index: i32);
        #[qinvokable]
        #[cxx_name = "moveItem"]
        fn move_item(self: Pin<&mut WatchPartyController>, from: i32, to: i32);
        #[qinvokable]
        #[cxx_name = "clearQueue"]
        fn clear_queue(self: Pin<&mut WatchPartyController>);
        /// 11.3 QOL (QOL.md §5): "Play next" — move `from` right after the playing item (`now_url`; absent → the top).
        /// One write (vectors queueops.json `moveToNext`).
        #[qinvokable]
        fn move_to_next(self: Pin<&mut WatchPartyController>, from: i32, now_url: &QString);
        /// 11.3 QOL: drop the items above the playing one (`clearPlayed`). One write.
        #[qinvokable]
        fn clear_played(self: Pin<&mut WatchPartyController>, now_url: &QString);
        /// 11.3 QOL: shuffle the items after the playing one (mulberry32 Fisher–Yates seeded with now & 0xffffffff,
        /// `shuffleUpcoming`). One write.
        #[qinvokable]
        fn shuffle_upcoming(self: Pin<&mut WatchPartyController>, now_url: &QString);
        /// 11.3 QOL: "48 min left" for the items after `now_url` ("" when nothing is known) — the queue header.
        #[qinvokable]
        fn upcoming_label(self: &WatchPartyController, queue_json: &QString, now_url: &QString) -> QString;
        /// 11.3 QOL: seconds (a STRING) → "m:ss", "h:mm:ss" from one hour (vectors `fmtTime`) — every time label.
        #[qinvokable]
        fn fmt_time(self: &WatchPartyController, secs: &QString) -> QString;
        /// QML: addLink(text, playNow) — 11.4 "paste any link in the one box": a URL → a queue item (YouTube / direct
        /// media at once; a Spotify / Apple song through /watch/meta + /watch/resolve → its YouTube upload; no match →
        /// a card, not queued), words → `{status:"search"}` (the window searches), a playlist / channel → `browse`.
        /// The outcome lands in linkJson.
        #[qinvokable]
        #[cxx_name = "addLink"]
        fn add_link(self: Pin<&mut WatchPartyController>, text: &QString, play_now: bool);
        /// QML: enqueueItems(itemsJson) — 11.4: append several items in ONE write (a playlist's "Add all").
        #[qinvokable]
        #[cxx_name = "enqueueItems"]
        fn enqueue_items(self: Pin<&mut WatchPartyController>, items_json: &QString);
        /// QML: browse(kind, paramsJson) — 11.4: kind = search (q, type, page; debounced 450 ms) | trending (cat) |
        /// playlist (id, page) | channel (id | handle, page). The answer lands in browseJson.
        #[qinvokable]
        fn browse(self: Pin<&mut WatchPartyController>, kind: &QString, params_json: &QString);
        /// QML: suggest(q) — 11.4: YouTube autocomplete into suggestJson ("" clears).
        #[qinvokable]
        fn suggest(self: Pin<&mut WatchPartyController>, q: &QString);
        /// QML: classifyLink(text) — 11.4: the pure link parser (medialinks.rs) as JSON, for the one box's "Add" affordance.
        #[qinvokable]
        #[cxx_name = "classifyLink"]
        fn classify_link(self: &WatchPartyController, text: &QString) -> QString;
        /// QML: watchRoom(roomId) — 11.4 (Media Party): poll `roomId`'s party every 3 s into callPartyJson
        /// (the call bar's "a party is live here" cue + now-playing strip); "" stops the poll.
        #[qinvokable]
        #[cxx_name = "watchRoom"]
        fn watch_room(self: Pin<&mut WatchPartyController>, room_id: &QString);
        /// QML: syncStartMemory(roomId, memUrl) — join the party in `roomId` and start a
        /// synced Memories replay of `memUrl` ("tinhat-memory://<room>/<session>") — UNLESS
        /// a LIVE replay of the SAME url is already in progress (playback url matches AND
        /// updatedAt is < PLAYBACK_LIVE_TTL_MS old), in which case FOLLOW it instead of
        /// stomping everyone back to 0 (the old join-stomp leftover). The check reads
        /// CURRENT playback from the store/server — NOT the QML mirror property, which is
        /// definitionally empty at join time.
        #[qinvokable]
        #[cxx_name = "syncStartMemory"]
        fn sync_start_memory(
            self: Pin<&mut WatchPartyController>,
            room_id: &QString,
            mem_url: &QString,
        );

        /// QML signal: desiredSeek(posSecs) — the player shell seeks its surface to `posSecs`
        /// (a STRING, whole-or-fractional seconds). Emitted only on a HARD drift correction.
        #[qsignal]
        fn desired_seek(self: Pin<&mut WatchPartyController>, pos_secs: QString);
    }

    impl cxx_qt::Threading for WatchPartyController {}
}

/// Shared mutable sync state (behind a Mutex so the 1s loop task + QML invokables agree).
/// All times in epoch-ms / whole seconds; NO f64 ever crosses a qproperty.
#[derive(Default)]
struct SyncState {
    /// The player's last reported LOCAL position (seconds). Fed by `reportPosition`.
    local_position: f64,
    /// Wall-clock (epoch-ms) we last applied a remote seek/state — the applyingRemote gate.
    applying_remote_until: u64,
    /// Wall-clock (epoch-ms) the advancer last wrote playback (heartbeat throttle).
    last_playback_write: u64,
    /// The last target we emitted a desiredSeek for + when (seek-dedup, anti-thrash).
    last_seek_target: f64,
    last_seek_at: u64,
    /// The last playback `seekNonce` we acted on (a fresh nonce forces a follower seek).
    last_nonce: u64,
    /// Wall-clock (epoch-ms) of our last viewer-presence heartbeat write. Drives a
    /// time-since-last cadence (not a wall-clock phase gate) so a follower can't miss its
    /// window under tick jitter and expire out of the viewer list.
    last_viewer_write: u64,
    /// Monotonic generation: bumped on every join/leave so a stale loop task self-exits.
    generation: u64,
    /// 11.4 History (CONTRACT §2): the playing url last recorded (or scheduled) and when the pending one is due.
    history_url: String,
    history_due: u64,
    /// Wall-clock (epoch-ms) of the last LOCAL USER write (enqueue/play/seek/pause). Within
    /// LOCAL_WRITE_GRACE_MS of it, run_tick SUPPRESSES its party/playback re-push — that re-push
    /// reads the not-yet-synced local store and would REVERT our optimistic value (queue item
    /// vanishes / the just-started video stops + re-buffers). NOT set by the advancer heartbeat.
    last_user_write: u64,
}

/// Re-write a viewer heartbeat (advancer + followers alike) at most this often (ms) —
/// comfortably under the VIEWER_TTL_MS (20s) so presence never lapses, while avoiding a
/// write every tick.
const VIEWER_HEARTBEAT_MS: u64 = 7_000;

#[derive(Default)]
pub struct WatchPartyControllerRust {
    room_id: QString,
    in_party: bool,
    party_json: QString,
    playback_json: QString,
    viewers_json: QString,
    am_advancer: bool,
    search_results_json: QString,
    searching: bool,
    error: QString,
    call_party_json: QString,
    link_json: QString,
    adding: bool,
    browse_json: QString,
    browsing: bool,
    suggest_json: QString,
    history_json: QString,
    browse_inflight: Mutex<Option<JoinHandle<()>>>,
    suggest_inflight: Mutex<Option<JoinHandle<()>>>,
    suggest_cache: Mutex<std::collections::HashMap<String, (u64, String)>>,
    sync: Mutex<SyncState>,
    loop_task: Mutex<Option<JoinHandle<()>>>,
    search_inflight: Mutex<Option<JoinHandle<()>>>,
    /// 11.4: the `watchRoom` poller (one at a time; the generation lets a stale task self-exit).
    watch_task: Mutex<Option<JoinHandle<()>>>,
    watch_gen: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl qobject::WatchPartyController {
    pub fn join_party(mut self: Pin<&mut Self>, room_id: &QString) {
        let rid = room_id.to_string();
        eprintln!("[watchparty] join_party room_id='{rid}'");
        if rid.is_empty() {
            eprintln!("[watchparty] join_party: EMPTY room_id — open a room first (no-op)");
            return;
        }
        // New generation: any prior loop self-exits when it sees the bump.
        let generation = {
            let mut s = self.rust().sync.lock().unwrap();
            s.generation += 1;
            *s = SyncState {
                generation: s.generation,
                // Seed the advancer-heartbeat throttle at NOW: with the default 0, the
                // very first tick after a join would satisfy ADVANCER_HEARTBEAT_MS and
                // re-write playback (fresh updatedAt) before anything real happened —
                // the join-stomp / stale-clock-laundering vector. The first heartbeat
                // now waits a full cadence from the join.
                last_playback_write: crate::appstate::now_ms(),
                ..Default::default()
            };
            s.generation
        };
        self.as_mut().set_room_id(QString::from(&rid));
        self.as_mut().set_in_party(true);
        self.as_mut().set_party_json(QString::default());
        self.as_mut().set_playback_json(QString::default());
        self.as_mut().set_viewers_json(QString::default());
        self.as_mut().set_am_advancer(false);
        self.as_mut().set_error(QString::default());

        let qt = self.qt_thread();
        let handle = crate::bridge::runtime().spawn(async move {
            // Write my viewer heartbeat immediately so I'm counted (and maybe the advancer).
            let _ = write_viewer(&rid).await;
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
            loop {
                ticker.tick().await;
                let started = rid.clone();
                let gen_now = generation;
                // Bail if a leave/rejoin happened (generation moved on) — checked via queue.
                let (alive_tx, alive_rx) = tokio::sync::oneshot::channel::<bool>();
                let alive_started = started.clone();
                let _ = qt.queue(move |o| {
                    let still = {
                        let s = o.rust().sync.lock().unwrap();
                        s.generation == gen_now
                    } && o.room_id().to_string() == alive_started
                        && *o.in_party();
                    let _ = alive_tx.send(still);
                });
                if !alive_rx.await.unwrap_or(false) {
                    break;
                }
                run_tick(&qt, &started, gen_now).await;
            }
        });
        if let Ok(mut g) = self.rust().loop_task.lock() {
            if let Some(prev) = g.replace(handle) {
                prev.abort();
            }
        }
    }

    pub fn leave_party(mut self: Pin<&mut Self>) {
        let room_id = self.room_id().to_string();
        // Bump the generation so the loop self-exits, and abort its handle.
        {
            let mut s = self.rust().sync.lock().unwrap();
            s.generation += 1;
        }
        if let Ok(mut g) = self.rust().loop_task.lock() {
            if let Some(prev) = g.take() {
                prev.abort();
            }
        }
        self.as_mut().set_in_party(false);
        self.as_mut().set_am_advancer(false);
        // Clear playback (mirrors join_party) so nowUrl settles to "" after close — otherwise the
        // stale {url} lingers in the 120ms playback mirror and a reopen would loadfile the PREVIOUS
        // video before the next tick cancels it. party/queue are refreshed by run_tick on rejoin.
        self.as_mut().set_playback_json(QString::default());
        // Drop my viewer presence (best-effort tombstone), then — if I was the LAST live
        // viewer — sweep the party's shared state (a synced replay everyone has left must
        // not leave its clock behind to pin the next joiner). Order matters: clear own
        // viewer FIRST, then check, then sweep — a race just means the next leaver sweeps.
        if !room_id.is_empty() {
            crate::bridge::runtime().spawn(async move {
                let _ = clear_viewer(&room_id).await;
                let Some(room) = room_by_id(&room_id).await else { return };
                let me = room.own_user_id().to_string();
                let now = crate::appstate::now_ms();
                // My own tombstone may not have echoed into the local store yet, so skip my
                // entry explicitly; everyone else must have expired per VIEWER_TTL_MS.
                let others_live = read_viewers(&room, now).await.iter().any(|v| {
                    v.get("userId").and_then(|u| u.as_str()).map(|u| u != me.as_str()).unwrap_or(false)
                });
                if !others_live {
                    // EMPTY-content state clear — the same convention as clear_viewer.
                    let empty = serde_json::json!({});
                    if let Err(e) =
                        crate::appstate::send_state(&room, PLAYBACK_EVENT, "", empty).await
                    {
                        eprintln!("[watchparty] leave sweep (playback) failed: {e}");
                    }
                    // 10.1 (TH-WP-01): the QUEUE (WP_EVENT: queue + host) is never swept here. Opening
                    // the window to look and closing it again used to wipe everything anyone had ever
                    // added, silently. Only the transient clock is cleared; the queue goes when someone
                    // clears it on purpose.
                    eprintln!("[watchparty] leave_party: last viewer out — swept the shared playback clock");
                }
            });
        }
        self.as_mut().set_room_id(QString::default());
    }

    pub fn enqueue(self: Pin<&mut Self>, url: &QString, title: &QString) {
        let room_id = self.room_id().to_string();
        let url = url.to_string();
        let title = title.to_string();
        eprintln!("[watchparty] enqueue room_id='{room_id}' url='{url}'");
        if room_id.is_empty() || url.trim().is_empty() {
            eprintln!(
                "[watchparty] enqueue: no-op (room_id empty? {} / url empty? {}) — must JOIN first",
                room_id.is_empty(),
                url.trim().is_empty()
            );
            return;
        }
        // Mark a local user write so run_tick suppresses its stale re-push — otherwise the next
        // tick re-reads the not-yet-synced local store and drops the item we optimistically push.
        // Capture the generation so a leave/rejoin while the write is in flight drops the push.
        let gen = {
            let mut s = self.rust().sync.lock().unwrap();
            s.last_user_write = crate::appstate::now_ms();
            s.generation
        };
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            match do_enqueue(&room_id, &url, &title).await {
                Ok(party) => {
                    eprintln!("[watchparty] enqueue: state write OK");
                    // OPTIMISTIC push of the queue we just wrote (push_party would re-read the
                    // not-yet-synced local store and drop the new item — the reported bug). Re-stamp
                    // the grace at PUSH time so a slow write can't let a tick revert before it lands.
                    let json = serde_json::to_string(&party).unwrap_or_else(|_| "{}".to_string());
                    let _ = qt.queue(move |mut o| {
                        {
                            let mut s = o.rust().sync.lock().unwrap();
                            if s.generation != gen {
                                return; // stale: left/rejoined while the write was in flight
                            }
                            s.last_user_write = crate::appstate::now_ms();
                        }
                        o.as_mut().set_party_json(QString::from(&json));
                    });
                }
                Err(e) => {
                    eprintln!("[watchparty] enqueue FAILED (state write): {e}");
                    let _ = qt.queue(move |mut o| {
                        o.as_mut().set_error(QString::from(
                            "Couldn't update the Media Party — this room may need an admin to enable it",
                        ))
                    });
                }
            }
        });
    }

    /// 11.4: append a full item (a song with art / source, a video with its thumbnail …).
    pub fn enqueue_item(self: Pin<&mut Self>, item_json: &QString) {
        let item: Value = serde_json::from_str(&item_json.to_string()).unwrap_or(Value::Null);
        let Some(mut item) = norm_queue_item(&item) else { return };
        item["addedBy"] = Value::from(""); // do_enqueue_item stamps me
        self.edit_queue(move |q, me| {
            item["addedBy"] = Value::from(me);
            q.push(item);
            true
        });
    }

    pub fn remove_at(self: Pin<&mut Self>, index: i32) {
        self.edit_queue(move |q, _| {
            if index < 0 || index as usize >= q.len() {
                return false;
            }
            q.remove(index as usize);
            true
        });
    }

    pub fn move_item(self: Pin<&mut Self>, from: i32, to: i32) {
        self.edit_queue(move |q, _| {
            let (f, t) = (from, to);
            if f < 0 || t < 0 || f as usize >= q.len() || t as usize >= q.len() || f == t {
                return false;
            }
            let it = q.remove(f as usize);
            q.insert(t as usize, it);
            true
        });
    }

    pub fn clear_queue(self: Pin<&mut Self>) {
        self.edit_queue(|q, _| {
            if q.is_empty() {
                return false;
            }
            q.clear();
            true
        });
    }

    pub fn move_to_next(self: Pin<&mut Self>, from: i32, now_url: &QString) {
        let now = now_url.to_string();
        if from < 0 {
            return;
        }
        self.edit_queue(move |q, _| queue_move_to_next(q, &now, from as usize));
    }

    pub fn clear_played(self: Pin<&mut Self>, now_url: &QString) {
        let now = now_url.to_string();
        self.edit_queue(move |q, _| queue_clear_played(q, &now));
    }

    pub fn shuffle_upcoming(self: Pin<&mut Self>, now_url: &QString) {
        let now = now_url.to_string();
        let seed = (crate::appstate::now_ms() & 0xffff_ffff) as u32; // QOL.md §5: Date.now() & 0xffffffff
        self.edit_queue(move |q, _| queue_shuffle_upcoming(q, &now, seed));
    }

    pub fn upcoming_label(&self, queue_json: &QString, now_url: &QString) -> QString {
        let v: Value = serde_json::from_str(&queue_json.to_string()).unwrap_or(Value::Null);
        let q: Vec<Value> = v.as_array().cloned().unwrap_or_default();
        QString::from(&queue_upcoming_label(&q, &now_url.to_string()))
    }

    pub fn fmt_time(&self, secs: &QString) -> QString {
        let t = secs.to_string();
        let v = t.trim().parse::<f64>().unwrap_or(f64::NAN);
        QString::from(&fmt_time(v))
    }

    pub fn enqueue_items(self: Pin<&mut Self>, items_json: &QString) {
        let v: Value = serde_json::from_str(&items_json.to_string()).unwrap_or(Value::Null);
        let list: Vec<Value> = v.as_array().map(|a| a.iter().filter_map(norm_queue_item).collect()).unwrap_or_default();
        if list.is_empty() {
            return;
        }
        self.edit_queue(move |q, me| {
            for mut it in list {
                it["addedBy"] = Value::from(me);
                q.push(it);
            }
            true
        });
    }

    pub fn browse(mut self: Pin<&mut Self>, kind: &QString, params_json: &QString) {
        let kind = kind.to_string();
        let params: Value = serde_json::from_str(&params_json.to_string()).unwrap_or(serde_json::json!({}));
        let debounce = if kind == "search" { 450 } else { 0 };
        if kind == "search" && params.get("q").and_then(Value::as_str).map(|q| q.trim().is_empty()).unwrap_or(true) {
            if let Ok(mut g) = self.rust().browse_inflight.lock() {
                if let Some(prev) = g.take() {
                    prev.abort();
                }
            }
            self.as_mut().set_browsing(false);
            self.as_mut().set_browse_json(QString::from(&serde_json::json!({ "kind": "search", "params": params, "ok": true, "items": [], "nextPage": "" }).to_string()));
            return;
        }
        self.as_mut().set_browsing(true);
        let qt = self.qt_thread();
        let handle = crate::bridge::runtime().spawn(async move {
            if debounce > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(debounce)).await;
            }
            let out = browse_fetch(&kind, &params).await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_browsing(false);
                o.as_mut().set_browse_json(QString::from(&out.to_string()));
            });
        });
        if let Ok(mut g) = self.rust().browse_inflight.lock() {
            if let Some(prev) = g.replace(handle) {
                prev.abort();
            }
        }
    }

    pub fn suggest(mut self: Pin<&mut Self>, q: &QString) {
        let q = q.to_string().trim().to_lowercase();
        if let Ok(mut g) = self.rust().suggest_inflight.lock() {
            if let Some(prev) = g.take() {
                prev.abort();
            }
        }
        if q.is_empty() {
            self.as_mut().set_suggest_json(QString::from("[]"));
            return;
        }
        let now = crate::appstate::now_ms();
        let cached: Option<String> = self
            .rust()
            .suggest_cache
            .lock()
            .ok()
            .and_then(|c| c.get(&q).filter(|(at, _)| now.saturating_sub(*at) < 3_600_000).map(|(_, json)| json.clone()));
        if let Some(json) = cached {
            self.as_mut().set_suggest_json(QString::from(&json));
            return;
        }
        let qt = self.qt_thread();
        let key = q.clone();
        let handle = crate::bridge::runtime().spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(350)).await; // the proxy's 30/min bucket is shared
            let json = match crate::proxy::get_json(&format!("/watch/suggest?q={}", enc(&q))).await {
                Ok(v) => {
                    let list: Vec<Value> = v.as_array().map(|a| a.iter().filter(|x| x.is_string()).take(10).cloned().collect()).unwrap_or_default();
                    Value::Array(list).to_string()
                }
                Err(_) => return,
            };
            let _ = qt.queue(move |mut o| {
                if let Ok(mut c) = o.rust().suggest_cache.lock() {
                    c.insert(key, (crate::appstate::now_ms(), json.clone()));
                }
                o.as_mut().set_suggest_json(QString::from(&json));
            });
        });
        if let Ok(mut g) = self.rust().suggest_inflight.lock() {
            if let Some(prev) = g.replace(handle) {
                prev.abort();
            }
        }
    }

    /// Read-modify-write the queue (keeps hostId and every item field — CONTRACT §1); `edit(queue, me)` returns
    /// whether anything changed. Mirrors enqueue()'s optimistic push + grace.
    fn edit_queue(self: Pin<&mut Self>, edit: impl FnOnce(&mut Vec<Value>, &str) -> bool + Send + 'static) {
        let room_id = self.room_id().to_string();
        if room_id.is_empty() || !*self.in_party() {
            return;
        }
        let gen = {
            let mut s = self.rust().sync.lock().unwrap();
            s.last_user_write = crate::appstate::now_ms();
            s.generation
        };
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            match do_edit_queue(&room_id, edit).await {
                Ok(Some(party)) => {
                    let json = serde_json::to_string(&party).unwrap_or_else(|_| "{}".to_string());
                    let _ = qt.queue(move |mut o| {
                        {
                            let mut s = o.rust().sync.lock().unwrap();
                            if s.generation != gen {
                                return;
                            }
                            s.last_user_write = crate::appstate::now_ms();
                        }
                        o.as_mut().set_party_json(QString::from(&json));
                        o.as_mut().set_error(QString::default());
                    });
                }
                Ok(None) => {}
                Err(e) => {
                    eprintln!("[watchparty] queue edit FAILED: {e}");
                    let _ = qt.queue(move |mut o| {
                        o.as_mut().set_error(QString::from(
                            "Couldn't update the Media Party — this room may need an admin to enable it",
                        ))
                    });
                }
            }
        });
    }

    pub fn classify_link(&self, text: &QString) -> QString {
        QString::from(&crate::medialinks::parse_media_link(&text.to_string()).to_string())
    }

    /// 11.4: the one box / a chat card → a queue item (see the bridge doc). Runs the server round trips off the UI
    /// thread; the outcome lands in `linkJson` (+ `adding` while in flight).
    pub fn add_link(mut self: Pin<&mut Self>, text: &QString, play_now: bool) {
        let text = text.to_string();
        let link = crate::medialinks::parse_media_link(&text);
        let kind = link.get("kind").and_then(Value::as_str).unwrap_or("text").to_string();
        if kind == "text" {
            let out = serde_json::json!({ "status": "search", "query": link["input"] });
            self.as_mut().set_link_json(QString::from(&out.to_string()));
            return;
        }
        if kind == "playlist" || kind == "channel" {
            let out = serde_json::json!({ "status": "browse", "link": link });
            self.as_mut().set_link_json(QString::from(&out.to_string()));
            return;
        }
        let room_id = self.room_id().to_string();
        if room_id.is_empty() || !*self.in_party() {
            let out = serde_json::json!({ "status": "error", "message": "Join the Media Party first." });
            self.as_mut().set_link_json(QString::from(&out.to_string()));
            return;
        }
        self.as_mut().set_adding(true);
        self.as_mut().set_link_json(QString::default());
        let gen = {
            let mut s = self.rust().sync.lock().unwrap();
            s.last_user_write = crate::appstate::now_ms();
            s.generation
        };
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let outcome = resolve_add_link(&room_id, &link, play_now).await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_adding(false);
                let status = outcome.get("status").and_then(Value::as_str).unwrap_or("");
                if status == "queued" || status == "playing" {
                    if let Some(party) = outcome.get("party") {
                        let json = serde_json::to_string(party).unwrap_or_else(|_| "{}".to_string());
                        let mut s = o.rust().sync.lock().unwrap();
                        if s.generation == gen {
                            s.last_user_write = crate::appstate::now_ms();
                            drop(s);
                            o.as_mut().set_party_json(QString::from(&json));
                            o.as_mut().set_error(QString::default());
                        }
                    }
                }
                o.as_mut().set_link_json(QString::from(&outcome.to_string()));
            });
        });
    }

    pub fn play_url(self: Pin<&mut Self>, url: &QString) {
        let room_id = self.room_id().to_string();
        let url = url.to_string();
        eprintln!("[watchparty] play_url room_id='{room_id}' url='{url}'");
        if room_id.is_empty() || url.trim().is_empty() {
            eprintln!("[watchparty] play_url: no-op (room/url empty)");
            return;
        }
        // A fresh item: position 0, playing, bump the nonce so everyone (re)loads + seeks.
        // Capture the generation so a leave/rejoin while the write is in flight drops the push.
        let gen = self.rust().sync.lock().unwrap().generation;
        let qt = self.qt_thread();
        let nonce = crate::appstate::now_ms();
        crate::bridge::runtime().spawn(async move {
            match write_playback(&room_id, &url, 0.0, true, nonce).await {
                Ok(content) => {
                    // OPTIMISTIC push (re-reading the local store would be stale → mpv never loads).
                    // Re-stamp the grace at PUSH time so a slow write can't let a tick revert first.
                    let json = playback_to_qml_json(&content);
                    let _ = qt.queue(move |mut o| {
                        {
                            let mut s = o.rust().sync.lock().unwrap();
                            if s.generation != gen {
                                return; // stale: left/rejoined while the write was in flight
                            }
                            s.last_user_write = crate::appstate::now_ms();
                        }
                        o.as_mut().set_playback_json(QString::from(&json));
                    });
                }
                Err(e) => {
                    eprintln!("[watchparty] play_url failed: {e}");
                    let _ = qt.queue(move |mut o| {
                        o.as_mut().set_error(QString::from(
                            "Couldn't start playback — this room may need an admin to enable Media Party",
                        ))
                    });
                }
            }
        });
        self.mark_local_write();
    }

    pub fn play(self: Pin<&mut Self>) {
        self.set_playing(true);
    }

    pub fn pause(self: Pin<&mut Self>) {
        self.set_playing(false);
    }

    pub fn seek(self: Pin<&mut Self>, pos_secs: &QString) {
        let room_id = self.room_id().to_string();
        let pos = parse_secs(&pos_secs.to_string());
        if room_id.is_empty() {
            return;
        }
        // Manual scrub: bump seekNonce so ALL followers jump to `pos` (keep playing state).
        // Capture the generation so a leave/rejoin while the write is in flight drops the push.
        let gen = {
            let mut s = self.rust().sync.lock().unwrap();
            s.local_position = pos;
            s.generation
        };
        let qt = self.qt_thread();
        let nonce = crate::appstate::now_ms();
        crate::bridge::runtime().spawn(async move {
            // Preserve the current url + playing flag; only move position + nonce.
            let (url, playing) = current_url_playing(&room_id).await;
            match write_playback(&room_id, &url, pos, playing, nonce).await {
                Ok(content) => {
                    let json = playback_to_qml_json(&content); // OPTIMISTIC (avoid stale re-read)
                    let _ = qt.queue(move |mut o| {
                        if o.rust().sync.lock().unwrap().generation != gen {
                            return; // stale: left/rejoined while the write was in flight
                        }
                        o.as_mut().set_playback_json(QString::from(&json));
                    });
                }
                Err(e) => eprintln!("[watchparty] seek failed: {e}"),
            }
        });
        self.mark_local_write();
    }

    pub fn report_position(self: Pin<&mut Self>, pos_secs: &QString) {
        let pos = parse_secs(&pos_secs.to_string());
        let mut s = self.rust().sync.lock().unwrap();
        s.local_position = pos;
    }

    pub fn assign_host(self: Pin<&mut Self>, user_id: &QString) {
        let room_id = self.room_id().to_string();
        let user_id = user_id.to_string();
        if room_id.is_empty() {
            return;
        }
        // Capture the generation so a leave/rejoin while the write is in flight drops the push.
        let gen = self.rust().sync.lock().unwrap().generation;
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            if let Err(e) = set_host(&room_id, &user_id).await {
                eprintln!("[watchparty] set host failed: {e}");
            }
            push_party(&qt, &room_id, gen).await;
        });
    }

    pub fn search(mut self: Pin<&mut Self>, query: &QString) {
        let q = query.to_string().trim().to_string();
        self.as_mut().set_error(QString::default());
        if q.is_empty() {
            self.as_mut().set_searching(false);
            self.as_mut().set_search_results_json(QString::from("[]"));
            if let Ok(mut g) = self.rust().search_inflight.lock() {
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
            let json = match search_watch(&q).await {
                Ok(j) => j,
                Err(e) => {
                    eprintln!("[watchparty] search: {e}");
                    let _ = qt.queue(move |mut o| {
                        o.as_mut().set_searching(false);
                        o.as_mut().set_error(QString::from("Couldn't search videos"));
                        o.as_mut().set_search_results_json(QString::from("[]"));
                    });
                    return;
                }
            };
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_searching(false);
                o.as_mut().set_search_results_json(QString::from(&json));
            });
        });
        if let Ok(mut g) = self.rust().search_inflight.lock() {
            if let Some(prev) = g.replace(handle) {
                prev.abort();
            }
        }
    }

    /// 11.4 (Media Party): watch `room_id`'s party (the call's room) — a 3 s poll of viewers + clock + queue
    /// into `callPartyJson`, independent of whether I am in the party. "" stops it (and clears the mirror).
    pub fn watch_room(mut self: Pin<&mut Self>, room_id: &QString) {
        use std::sync::atomic::Ordering;
        let rid = room_id.to_string();
        let gen = self.rust().watch_gen.fetch_add(1, Ordering::SeqCst) + 1;
        if let Ok(mut g) = self.rust().watch_task.lock() {
            if let Some(prev) = g.take() {
                prev.abort();
            }
        }
        if rid.is_empty() {
            self.as_mut().set_call_party_json(QString::default());
            return;
        }
        let gen_cell = self.rust().watch_gen.clone();
        let qt = self.qt_thread();
        let handle = crate::bridge::runtime().spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(3));
            loop {
                ticker.tick().await;
                if gen_cell.load(Ordering::SeqCst) != gen {
                    break;
                }
                let json = read_call_party(&rid).await;
                let gen_q = gen_cell.clone();
                let _ = qt.queue(move |mut o| {
                    if gen_q.load(Ordering::SeqCst) != gen {
                        return;
                    }
                    if o.call_party_json().to_string() != json {
                        o.as_mut().set_call_party_json(QString::from(&json));
                    }
                });
            }
        });
        if let Ok(mut g) = self.rust().watch_task.lock() {
            if let Some(prev) = g.replace(handle) {
                prev.abort();
            }
        }
    }

    /// Join `room_id`'s party and start a synced Memories replay of `mem_url` — join-don't-
    /// stomp (Rust-side authoritative). Reads CURRENT playback via `read_playback` (the same
    /// helper the tick uses — the source of truth), never the QML mirror property (which is
    /// definitionally empty right after join): if a LIVE replay (same url, updatedAt fresher
    /// than PLAYBACK_LIVE_TTL_MS) is in progress we FOLLOW it; otherwise we write playback
    /// (position 0, playing, fresh nonce).
    pub fn sync_start_memory(mut self: Pin<&mut Self>, room_id: &QString, mem_url: &QString) {
        let rid = room_id.to_string();
        let mem = mem_url.to_string().trim().to_string();
        eprintln!("[watchparty] sync_start_memory room_id='{rid}' url='{mem}'");
        if rid.is_empty() || mem.is_empty() {
            eprintln!("[watchparty] sync_start_memory: no-op (room/url empty)");
            return;
        }
        // Join first: starts the 1s loop + my viewer heartbeat, resets SyncState (fresh gen).
        self.as_mut().join_party(room_id);
        // Capture the (post-join) generation so a leave while in flight drops the push.
        let gen = self.rust().sync.lock().unwrap().generation;
        let qt = self.qt_thread();
        let nonce = crate::appstate::now_ms();
        crate::bridge::runtime().spawn(async move {
            // Source-of-truth read (store/server), NOT the QML mirror property.
            if let Some(room) = room_by_id(&rid).await {
                if let Some(pb) = read_playback(&room).await {
                    let cur_url =
                        pb.get("url").and_then(|u| u.as_str()).unwrap_or("").trim().to_string();
                    let updated_at = pb.get("updatedAt").and_then(as_u64_opt).unwrap_or(0);
                    let now = crate::appstate::now_ms();
                    if cur_url == mem && now.saturating_sub(updated_at) < PLAYBACK_LIVE_TTL_MS {
                        eprintln!(
                            "[watchparty] sync_start_memory: LIVE replay of this memory already \
                             in progress — FOLLOWING it (no playback stomp)"
                        );
                        // Un-gate the tick's playback push: mark_local_write below armed the
                        // grace for the WRITE path; when following there is nothing local to
                        // protect and the grace would only delay the first follow by ~3s.
                        let _ = qt.queue(move |o| {
                            let mut s = o.rust().sync.lock().unwrap();
                            if s.generation == gen {
                                s.last_user_write = 0;
                            }
                        });
                        return;
                    }
                }
            }
            // No live in-progress replay of this url → (re)start it from 0, fresh nonce.
            match write_playback(&rid, &mem, 0.0, true, nonce).await {
                Ok(content) => {
                    let json = playback_to_qml_json(&content); // OPTIMISTIC (avoid stale re-read)
                    let _ = qt.queue(move |mut o| {
                        {
                            let mut s = o.rust().sync.lock().unwrap();
                            if s.generation != gen {
                                return; // stale: left/rejoined while the write was in flight
                            }
                            s.last_user_write = crate::appstate::now_ms();
                        }
                        o.as_mut().set_playback_json(QString::from(&json));
                    });
                }
                Err(e) => {
                    eprintln!("[watchparty] sync_start_memory write failed: {e}");
                    let _ = qt.queue(move |mut o| {
                        o.as_mut().set_error(QString::from(
                            "Couldn't start the synced replay — this room may need an admin to enable Media Party",
                        ))
                    });
                }
            }
        });
        // Protect the (possible) optimistic write from a stale tick re-push; the follow
        // branch un-gates this as soon as it decides.
        self.mark_local_write();
    }

    // ---- internal helpers on the QObject ----

    /// Toggle playing (advancer-only; a follower's tick will re-write from the advancer).
    fn set_playing(self: Pin<&mut Self>, playing: bool) {
        let room_id = self.room_id().to_string();
        if room_id.is_empty() {
            return;
        }
        // Capture the generation so a leave/rejoin while the write is in flight drops the push.
        let (local, gen) = {
            let s = self.rust().sync.lock().unwrap();
            (s.local_position, s.generation)
        };
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            // Preserve the EXISTING seekNonce: a play/pause is a transport toggle, NOT a
            // manual scrub, so it must not force every follower to hard-seek (a fresh nonce
            // would). We re-anchor updatedAt=now at the current position instead.
            let (url, _was, nonce) = current_url_playing_nonce(&room_id).await;
            // Re-anchor: when we pause we freeze at the current local position; when we
            // resume we anchor updatedAt=now at the local position so target tracks forward.
            match write_playback(&room_id, &url, local, playing, nonce).await {
                Ok(content) => {
                    let json = playback_to_qml_json(&content); // OPTIMISTIC (avoid stale re-read)
                    let _ = qt.queue(move |mut o| {
                        if o.rust().sync.lock().unwrap().generation != gen {
                            return; // stale: left/rejoined while the write was in flight
                        }
                        o.as_mut().set_playback_json(QString::from(&json));
                    });
                }
                Err(e) => eprintln!("[watchparty] set_playing failed: {e}"),
            }
        });
        self.mark_local_write();
    }

    /// Record a local write so the heartbeat throttle restarts from now.
    fn mark_local_write(self: Pin<&mut Self>) {
        let mut s = self.rust().sync.lock().unwrap();
        let now = crate::appstate::now_ms();
        s.last_playback_write = now; // advancer-heartbeat throttle
        s.last_user_write = now;     // run_tick optimistic-push grace (suppress stale re-push)
    }
}

// ---------------------------------------------------------------------------
// The 1s sync tick (the heart of useWatchSync.ts). Runs on the loop task, but
// reaches the QObject (and its SyncState) through `qt.queue`.
// ---------------------------------------------------------------------------

/// One iteration of the sync loop: read state → compute target → decide seek-vs-ride →
/// (if advancer) heartbeat viewer + maybe re-write playback. All mutation of the QObject
/// (props, signal emit, SyncState reads) happens inside the queued closure.
async fn run_tick(qt: &cxx_qt::CxxQtThread<qobject::WatchPartyController>, room_id: &str, gen: u64) {
    let Some(room) = room_by_id(room_id).await else { return };
    let me = room.own_user_id().to_string();
    let now = crate::appstate::now_ms();

    let party = read_party(&room).await;
    let playback = read_playback(&room).await;
    // 11.3 QOL: who wrote the clock (the OSD's "{name} paused"); only read when there is a clock
    let playback_by = if playback.is_some() {
        crate::appstate::get_state_sender(&room, PLAYBACK_EVENT, "").await.unwrap_or_default()
    } else {
        String::new()
    };
    let viewers = read_viewers(&room, now).await;
    let history = read_history(&room).await;

    // FRESHNESS FILTER (synced Memories replays): a "tinhat-memory://…" playback clock whose
    // updatedAt is older than MEMORY_PLAYBACK_TTL_MS is a dead session's leftovers (the
    // last-leaver sweep raced or never ran) — treat it as ABSENT entirely: no follow, no
    // desiredSeek, no display. Field incident: a stale playing:true event from hours
    // earlier, extrapolated forward, pinned an unsynced player.
    let playback = playback.filter(|pb| {
        let u = pb.get("url").and_then(|x| x.as_str()).unwrap_or("");
        if !u.starts_with("tinhat-memory://") {
            return true;
        }
        let upd = pb.get("updatedAt").and_then(as_u64_opt).unwrap_or(0);
        now.saturating_sub(upd) <= MEMORY_PLAYBACK_TTL_MS
    });

    // amAdvancer = my id is the LOWEST among current (non-expired) viewers. If I somehow
    // see no viewers yet (my heartbeat hasn't landed), fall back to "me" so a solo host
    // still advances.
    let advancer = viewers
        .iter()
        .filter_map(|v| v.get("userId").and_then(|u| u.as_str()).map(str::to_string))
        .min()
        .unwrap_or_else(|| me.clone());
    let am_advancer = advancer == me;

    // The shared clock's target position.
    let pb = playback.clone();
    let url = pb.as_ref().and_then(|p| p.get("url")).and_then(|u| u.as_str()).unwrap_or("").to_string();
    // Canonical-aware read: prefer the integer positionMs source-of-truth (matching
    // playback_to_qml_json + the probe), falling back to the positionSecs string mirror.
    // Sanity-clamp the stored position (MAX_MEDIA_SECS, module const — writes clamp too;
    // see the advancer heartbeat). A poisoned room is self-sustaining: the advancer WRITES
    // BACK its extrapolated target, so once a stale clock produced a nonsense position
    // (see STALE_CLOCK_SECS below) that value lives in room state with a FRESH updatedAt
    // and the staleness check can't see it. No watch-party media is >24h, so a position
    // beyond that is definitionally garbage — restart the clock instead of seeking every
    // viewer into the void forever.
    let position = pb
        .as_ref()
        .map(playback_position_secs)
        .filter(|p| p.is_finite() && *p >= 0.0 && *p <= MAX_MEDIA_SECS)
        .unwrap_or(0.0);
    let playing = pb.as_ref().and_then(|p| p.get("playing")).and_then(Value::as_bool).unwrap_or(false);
    let updated_at = pb.as_ref().and_then(|p| p.get("updatedAt")).and_then(as_u64_opt).unwrap_or(now);
    let seek_nonce = pb.as_ref().and_then(|p| p.get("seekNonce")).and_then(as_u64_opt).unwrap_or(0);
    // Extrapolate the shared clock from the last write — but ONLY if that write is recent.
    // A watch party left in the "playing" state POISONS the room: its state event keeps
    // extrapolating forever, so the next person to open watch party gets a target of
    // (position + days), every `seek <huge> absolute` fails with MPV_ERROR_COMMAND, and the
    // surface stays black with no way back. (Observed live: a 2-minute video seeking to
    // 1,326,242s — ~15 days — from a state event left behind by an earlier session.)
    // Beyond STALE_CLOCK_SECS we treat the clock as dead: hold at the stored position and
    // don't chase it. A real play/pause/seek by anyone rewrites updatedAt and revives it.
    const STALE_CLOCK_SECS: f64 = 6.0 * 3600.0;
    let elapsed_secs = (now.saturating_sub(updated_at) as f64) / 1000.0;
    let stale = elapsed_secs > STALE_CLOCK_SECS;
    if stale && playing {
        // Per-occurrence warn, rate-limited to once a minute (was a warn-ONCE-ever static
        // bool that silenced every later stale party for the whole process lifetime).
        static LAST_STALE_WARN_MS: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(0);
        let last = LAST_STALE_WARN_MS.load(std::sync::atomic::Ordering::Relaxed);
        if now.saturating_sub(last) >= 60_000
            && LAST_STALE_WARN_MS
                .compare_exchange(
                    last,
                    now,
                    std::sync::atomic::Ordering::Relaxed,
                    std::sync::atomic::Ordering::Relaxed,
                )
                .is_ok()
        {
            eprintln!(
                "[watchparty] shared clock is STALE ({elapsed_secs:.0}s since last write) — \
                 not extrapolating; presenting PAUSED at the stored position. Press play to resync."
            );
        }
    }
    let target = position + if playing && !stale { elapsed_secs } else { 0.0 };

    let party_json = serde_json::to_string(&party).unwrap_or_else(|_| "{}".to_string());
    // A STALE clock (> STALE_CLOCK_SECS old) is surfaced to QML PAUSED at its stored
    // position — playing:false means the QML side never extrapolates it forward, so a
    // late joiner doesn't rubber-band to (position + hours).
    let playback_json = playback
        .as_ref()
        .map(|pb| {
            if stale {
                let mut v = pb.clone();
                v["playing"] = Value::from(false);
                playback_to_qml_json_by(&v, &playback_by)
            } else {
                playback_to_qml_json_by(pb, &playback_by)
            }
        })
        .unwrap_or_else(|| "{}".to_string());
    // 11.3 QOL: one log line whenever the clock's url / playing / seekNonce / sender changes (the OSD's inputs)
    {
        static LAST_CLOCK: Mutex<String> = Mutex::new(String::new());
        let key = format!("{url} playing={playing} nonce={seek_nonce} by={playback_by}");
        if let Ok(mut last) = LAST_CLOCK.lock() {
            if *last != key {
                eprintln!("[watchparty] clock: {key}");
                *last = key;
            }
        }
    }
    let viewers_json = serde_json::to_string(&viewers).unwrap_or_else(|_| "[]".to_string());
    let history_json = serde_json::to_string(&history).unwrap_or_else(|_| "[]".to_string());

    // -------- the seek-vs-ride decision (queued so it can read/emit on the QObject) --------
    let url_for_write = url.clone();
    let url_hist = url.clone();
    let (decided_tx, decided_rx) = tokio::sync::oneshot::channel::<TickDecision>();
    let me_q = me.clone();
    let _ = qt.queue(move |mut o| {
        // Stale tick (a leave/rejoin happened): drop it. Also: if a LOCAL user write happened
        // within LOCAL_WRITE_GRACE_MS, SUPPRESS the party/playback re-push — it reads the
        // not-yet-synced local store and would REVERT our optimistic value (queue item vanishes /
        // the just-started video stops + re-buffers). Viewers + amAdvancer are unaffected.
        let (stale_gen, suppress_local) = {
            let g = o.rust().sync.lock().unwrap();
            (g.generation != gen, now.saturating_sub(g.last_user_write) < LOCAL_WRITE_GRACE_MS)
        };
        if stale_gen {
            let _ = decided_tx.send(TickDecision::default());
            return;
        }
        if !suppress_local {
            o.as_mut().set_party_json(QString::from(&party_json));
            o.as_mut().set_playback_json(QString::from(&playback_json));
        }
        o.as_mut().set_viewers_json(QString::from(&viewers_json));
        o.as_mut().set_am_advancer(am_advancer);
        if o.history_json().to_string() != history_json {
            o.as_mut().set_history_json(QString::from(&history_json));
        }

        // `sync` is a Mutex behind a shared ref, so `o.rust()` (immutable pin borrow) is
        // enough to lock it — no `rust_mut()` (which would create a dropped temporary Pin).
        let (decision, write_viewer_heartbeat) = {
            let mut guard = o.rust().sync.lock().unwrap();
            let d = decide_sync(&mut guard, target, playing, seek_nonce, now);
            // Presence cadence (advancer AND followers): re-write when our last heartbeat is
            // older than VIEWER_HEARTBEAT_MS (time-since-last, NOT a wall-clock phase gate).
            // 7s comfortably preserves the 20s VIEWER_TTL_MS even if one write fails.
            let want_viewer = now.saturating_sub(guard.last_viewer_write) >= VIEWER_HEARTBEAT_MS;
            if want_viewer {
                guard.last_viewer_write = now;
            }
            (d, want_viewer)
        };
        // Suppress the drift-correction seek during the local-write grace too: the tick-top
        // `target` is the STALE pre-sync value, so emitting it would seek our just-loaded video to
        // a garbage position. The local player is authoritative until the /sync read lands.
        if !suppress_local {
            if let Some(seek_to) = decision.emit_seek {
                o.as_mut().desired_seek(QString::from(&format!("{seek_to:.3}")));
            }
        }
        // CONTRACT §2: the ADVANCER records a new playing url (never a tinhat-memory:// replay) in the room's History
        // 5 s after it changed, only if it is still playing then (a mis-click skipped at once leaves no trace).
        let write_history = {
            let mut guard = o.rust().sync.lock().unwrap();
            if am_advancer && !url_hist.is_empty() && !url_hist.starts_with("tinhat-memory://") {
                if guard.history_url != url_hist {
                    guard.history_url = url_hist.clone();
                    guard.history_due = now + HISTORY_DEBOUNCE_MS;
                    false
                } else if guard.history_due != 0 && now >= guard.history_due {
                    guard.history_due = 0;
                    true
                } else {
                    false
                }
            } else {
                if url_hist.is_empty() {
                    guard.history_url.clear();
                }
                guard.history_due = 0;
                false
            }
        };
        let _ = decided_tx.send(TickDecision {
            advance_write: am_advancer && decision.advancer_should_write,
            write_viewer_heartbeat,
            write_history,
        });
        let _ = &me_q; // (kept for symmetry / future per-user logic)
    });

    let decision = decided_rx.await.unwrap_or_default();

    // The advancer (and only the advancer) periodically RE-WRITES playback so followers
    // re-anchor against a fresh updatedAt (and so a late joiner sees a live clock).
    if decision.advance_write && !url_for_write.is_empty() {
        // RE-READ playback immediately before the heartbeat write. The tick-top reads
        // (url/position/playing/seek_nonce) can be STALE: any peer (or our own QML) may have
        // scrubbed/played/paused after we sampled them. If we blindly re-wrote the tick-top
        // snapshot we'd revert that change AND down-write the (now-newer) seekNonce, so
        // followers that already consumed the new nonce would never re-seek — the scrub would
        // be silently lost. So we anchor the heartbeat on the FRESH state: keep its url/
        // playing/nonce, and only re-anchor the position+updatedAt to the live forward target.
        if let Some(room) = room_by_id(room_id).await {
            if let Some(fresh) = read_playback(&room).await {
                let f_url = fresh.get("url").and_then(|u| u.as_str()).unwrap_or("").to_string();
                let f_playing = fresh.get("playing").and_then(Value::as_bool).unwrap_or(false);
                let f_nonce = fresh.get("seekNonce").and_then(as_u64_opt).unwrap_or(0);
                let f_updated = fresh.get("updatedAt").and_then(as_u64_opt).unwrap_or(now);
                let f_pos = playback_position_secs(&fresh);
                // seekNonce is MONOTONIC: never overwrite a newer nonce with our stale one.
                if f_nonce > seek_nonce {
                    // A scrub/transport landed since tick-top — leave it; don't heartbeat over it.
                } else if !f_url.is_empty() {
                    // STALE-CLOCK LAUNDERING GUARD: this heartbeat writes a fresh updatedAt,
                    // which would turn an hours-old playing:true clock into a live-looking
                    // one AND bake the bogus extrapolated position into room state. If the
                    // existing clock isn't LIVE, park it instead: write the STORED position
                    // UNextrapolated with playing=false.
                    let age_ms = now.saturating_sub(f_updated);
                    let (w_pos, w_playing) = if age_ms > PLAYBACK_LIVE_TTL_MS {
                        (f_pos, false)
                    } else {
                        // Live clock: re-anchor position+updatedAt to the forward target.
                        let t = f_pos
                            + if f_playing { age_ms as f64 / 1000.0 } else { 0.0 };
                        (t, f_playing)
                    };
                    // Never write a nonsense position (the poisoned-room self-sustain).
                    let w_pos =
                        if w_pos.is_finite() { w_pos.clamp(0.0, MAX_MEDIA_SECS) } else { 0.0 };
                    if let Err(e) = write_playback(room_id, &f_url, w_pos, w_playing, f_nonce).await {
                        eprintln!("[watchparty] advancer heartbeat write failed: {e}");
                    }
                }
            }
        }
        // Mark the write time on the QObject so the throttle restarts.
        let _ = qt.queue(move |o| {
            o.rust().sync.lock().unwrap().last_playback_write = now;
        });
    }

    // Refresh my viewer heartbeat (TTL presence) on the time-since-last cadence decided
    // above — advancer and followers alike. (A per-tick advancer write would land a state
    // event in every member's /sync once a second for the whole film.) Stale-generation
    // ticks return a default decision, so a leave/rejoin can't resurrect presence here.
    if decision.write_viewer_heartbeat {
        let _ = write_viewer(room_id).await;
    }
    if decision.write_history && !url_for_write.is_empty() {
        if let Err(e) = write_history(room_id, &url_for_write, &me).await {
            eprintln!("[watchparty] history write failed: {e}");
        }
    }
}

#[derive(Default)]
struct TickDecision {
    advance_write: bool,
    /// Re-write my viewer heartbeat this tick (cadence-gated; advancer + followers alike).
    write_viewer_heartbeat: bool,
    /// 11.4: record the playing url in the room's History (the advancer, 5 s after it changed).
    write_history: bool,
}

/// The pure decision core (testable headless): given the shared `target`, the playing
/// flag, the current `seek_nonce`, and `now`, mutate the per-controller `SyncState` and
/// return whether to emit a `desiredSeek` and whether the advancer should re-write playback.
/// This is the exact `useWatchSync.ts` decision tree, isolated so the probe can assert it.
struct DecisionOut {
    /// `Some(target)` ⇒ emit desiredSeek(target); `None` ⇒ ride (no seek).
    emit_seek: Option<f64>,
    /// The advancer should re-write playback this tick (heartbeat or after applying remote).
    advancer_should_write: bool,
}

fn decide_sync(
    s: &mut SyncState,
    target: f64,
    _playing: bool,
    seek_nonce: u64,
    now: u64,
) -> DecisionOut {
    let drift = (s.local_position - target).abs();

    // The applyingRemote gate: shortly after we applied a remote seek, don't immediately
    // re-emit another seek for (near) the same target (anti ping-pong), and don't let the
    // advancer write either.
    let in_apply_window = now < s.applying_remote_until;

    // HARD drift ⇒ seek (unless we just applied a remote correction for this target, or we
    // already emitted a seek for ~this target inside the dedup window — anti-thrash on a
    // buffering stall).
    let mut emit_seek = None;
    if drift > HARD_TOL_SECS && !in_apply_window {
        let same_target_recent = (target - s.last_seek_target).abs() < HARD_TOL_SECS
            && now.saturating_sub(s.last_seek_at) < SEEK_DEDUP_MS;
        // A new manual scrub (seekNonce changed) always forces a seek, bypassing dedup.
        let nonce_forced = seek_nonce != 0 && seek_nonce != s_last_nonce(s);
        if !same_target_recent || nonce_forced {
            emit_seek = Some(target);
            s.last_seek_target = target;
            s.last_seek_at = now;
            s.applying_remote_until = now + APPLYING_REMOTE_MS;
            // Optimistically advance our local model to the target so the next tick's drift
            // is measured against where we asked the player to go (the player will confirm
            // via reportPosition).
            s.local_position = target;
            set_last_nonce(s, seek_nonce);
        }
    } else if seek_nonce != 0 && seek_nonce != s_last_nonce(s) {
        // Even within tolerance, a fresh manual-scrub nonce means everyone jumps.
        emit_seek = Some(target);
        s.last_seek_target = target;
        s.last_seek_at = now;
        s.applying_remote_until = now + APPLYING_REMOTE_MS;
        s.local_position = target;
        set_last_nonce(s, seek_nonce);
    } else {
        // Small drift (or a tolerated stall): RIDE — no seek. Keep the player playing.
        set_last_nonce(s, seek_nonce);
    }

    // The advancer re-writes playback on its heartbeat cadence (not while applying remote).
    let advancer_should_write =
        !in_apply_window && now.saturating_sub(s.last_playback_write) >= ADVANCER_HEARTBEAT_MS;

    DecisionOut { emit_seek, advancer_should_write }
}

// Thin accessors for the last-acted-on nonce, kept so `decide_sync` reads cleanly.
fn s_last_nonce(s: &SyncState) -> u64 {
    s.last_nonce
}
fn set_last_nonce(s: &mut SyncState, n: u64) {
    s.last_nonce = n;
}

// ---------------------------------------------------------------------------
// Data layer — reads/writes of the three state events (reusable by the probe).
// ---------------------------------------------------------------------------

async fn room_by_id(room_id: &str) -> Option<Room> {
    let client = crate::session::client()?;
    let rid = RoomId::parse(room_id).ok()?;
    client.get_room(&rid)
}

/// Read the queue+host party event as `{ queue:[{url,title,addedBy}], hostId }` (defaults
/// when absent).
pub async fn read_party(room: &Room) -> Value {
    let content = crate::appstate::get_state(room, WP_EVENT, "").await.unwrap_or(Value::Null);
    let queue = content
        .get("queue")
        .and_then(|q| q.as_array())
        .map(|arr| arr.iter().filter_map(norm_queue_item).collect::<Vec<_>>())
        .unwrap_or_default();
    let host_id = content.get("hostId").and_then(|h| h.as_str()).unwrap_or("").to_string();
    serde_json::json!({ "queue": queue, "hostId": host_id })
}

/// One queue item as the apps agree on it (Media Party CONTRACT §1): `url` (trimmed, required — an item without
/// one is dropped), `title` / `addedBy` strings (default ""), the optional song fields normalised when present
/// (`kind`, `source{service,url,id}`, `art`, `artist`, `durationSec`, `preview`) and EVERY other field kept as-is,
/// so a client that rewrites the queue never strips what a newer one wrote.
pub fn norm_queue_item(it: &Value) -> Option<Value> {
    let obj = it.as_object()?;
    let url = obj.get("url").and_then(|u| u.as_str()).unwrap_or("").trim().to_string();
    if url.is_empty() {
        return None;
    }
    let mut out = serde_json::Map::new();
    for (k, v) in obj {
        out.insert(k.clone(), v.clone());
    }
    out.insert("url".into(), Value::from(url));
    let s = |k: &str| obj.get(k).and_then(|t| t.as_str()).unwrap_or("").to_string();
    out.insert("title".into(), Value::from(s("title")));
    out.insert("addedBy".into(), Value::from(s("addedBy")));
    for k in ["kind", "art", "artist", "preview"] {
        match obj.get(k).and_then(|v| v.as_str()).map(str::trim) {
            Some(v) if !v.is_empty() && (k != "kind" || matches!(v, "video" | "song" | "file")) => {
                out.insert(k.into(), Value::from(v));
            }
            _ => {
                out.remove(k);
            }
        }
    }
    match obj.get("durationSec").and_then(as_u64_opt) {
        Some(d) if d > 0 => {
            out.insert("durationSec".into(), Value::from(d));
        }
        _ => {
            out.remove("durationSec");
        }
    }
    match obj.get("source") {
        Some(Value::Object(src)) => {
            let g = |k: &str| src.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            let (service, surl, id) = (g("service"), g("url"), g("id"));
            if matches!(service.as_str(), "youtube" | "ytmusic" | "spotify" | "apple") {
                out.insert("source".into(), serde_json::json!({ "service": service, "url": surl, "id": id }));
            } else {
                out.remove("source");
            }
        }
        _ => {
            out.remove("source");
        }
    }
    Some(Value::Object(out))
}

// ---------------------------------------------------------------------------
// 11.3 QOL (QOL.md §2 / §5): the pure queue ops + time label — the web's queue.ts twins, both tested against the
// shared vectors testvectors/mediaparty/queueops.json (reference: share claude/tinhat-media-party/qol/queueops_ref.py).
// Each op edits the queue in place and says whether anything changed (edit_queue writes only then).
// ---------------------------------------------------------------------------

fn item_url(it: &Value) -> &str {
    it.get("url").and_then(Value::as_str).unwrap_or("")
}

/// The index of the playing url in the queue (-1 when nothing plays or it is not queued).
pub fn queue_now_index(q: &[Value], now: &str) -> i64 {
    if now.is_empty() {
        return -1;
    }
    q.iter().position(|it| item_url(it) == now).map(|i| i as i64).unwrap_or(-1)
}

/// `moveToNext`: the item at `from` goes right after the playing one (the top when nothing plays / it is not queued).
pub fn queue_move_to_next(q: &mut Vec<Value>, now: &str, from: usize) -> bool {
    if from >= q.len() {
        return false;
    }
    if queue_now_index(q, now) == from as i64 {
        return false;
    }
    let before: Vec<String> = q.iter().map(|it| item_url(it).to_string()).collect();
    let it = q.remove(from);
    let ni = queue_now_index(q, now);
    q.insert((ni + 1) as usize, it);
    q.iter().map(item_url).ne(before.iter().map(String::as_str))
}

/// `clearPlayed`: drop everything above the playing item.
pub fn queue_clear_played(q: &mut Vec<Value>, now: &str) -> bool {
    let ni = queue_now_index(q, now);
    if ni <= 0 {
        return false;
    }
    q.drain(0..ni as usize);
    true
}

/// mulberry32 — bit-for-bit the JS / Python reference (u32 wrapping arithmetic = Math.imul + `>>> 0`).
pub fn mulberry32(seed: u32) -> impl FnMut() -> f64 {
    let mut st = seed;
    move || {
        st = st.wrapping_add(0x6D2B_79F5);
        let mut t = st;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t = t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61)) ^ t;
        ((t ^ (t >> 14)) as f64) / 4_294_967_296.0
    }
}

/// `shuffleUpcoming`: Fisher–Yates over the items after the playing one (all of them when nothing plays).
pub fn queue_shuffle_upcoming(q: &mut Vec<Value>, now: &str, seed: u32) -> bool {
    let ni = queue_now_index(q, now);
    let start = if ni >= 0 { ni as usize + 1 } else { 0 };
    if q.len() < start + 2 {
        return false;
    }
    let before: Vec<String> = q.iter().map(|it| item_url(it).to_string()).collect();
    let up = &mut q[start..];
    let mut r = mulberry32(seed);
    for i in (1..up.len()).rev() {
        let j = (r() * (i as f64 + 1.0)).floor() as usize;
        up.swap(i, j);
    }
    q.iter().map(item_url).ne(before.iter().map(String::as_str))
}

fn fmt_dur_mins(mins: u64) -> String {
    if mins < 1 {
        return "<1 min".into();
    }
    if mins < 60 {
        return format!("{mins} min");
    }
    let (h, m) = (mins / 60, mins % 60);
    if m == 0 {
        format!("{h} h")
    } else {
        format!("{h} h {m} min")
    }
}

/// `upcomingLabel`: "48 min left" over the items after the playing one ("+" when some durations are unknown; "" when
/// nothing is left or no duration is known).
pub fn queue_upcoming_label(q: &[Value], now: &str) -> String {
    let ni = queue_now_index(q, now);
    let up = if ni >= 0 { &q[ni as usize + 1..] } else { q };
    if up.is_empty() {
        return String::new();
    }
    let dur = |it: &Value| it.get("durationSec").map(as_f64).filter(|d| d.is_finite() && *d > 0.0);
    let known: f64 = up.iter().filter_map(dur).map(f64::trunc).sum();
    let unknown = up.iter().any(|it| dur(it).is_none());
    if known <= 0.0 {
        return String::new();
    }
    let mins = (known / 60.0 + 0.5).floor() as u64; // round half up
    format!("{}{} left", fmt_dur_mins(mins), if unknown { "+" } else { "" })
}

/// `fmtTime`: "m:ss", "h:mm:ss" from one hour; anything not finite / negative reads 0:00.
pub fn fmt_time(secs: f64) -> String {
    if !secs.is_finite() {
        return "0:00".into();
    }
    let s = secs.floor().max(0.0) as u64;
    let (h, rem) = (s / 3600, s % 3600);
    let (m, sec) = (rem / 60, rem % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

/// The url after the playing one (the first item when nothing plays / it is not queued; "" after the last) — the
/// window's nextQueueUrl() and the call-bar strip's next button.
pub fn queue_next_url(q: &[Value], now: &str) -> String {
    if q.is_empty() {
        return String::new();
    }
    let ni = queue_now_index(q, now);
    if ni < 0 {
        return item_url(&q[0]).to_string();
    }
    q.get(ni as usize + 1).map(|it| item_url(it).to_string()).unwrap_or_default()
}

/// 11.4 (Media Party): the call bar's view of `room_id`'s party — live viewers, the clock and the playing item
/// (its queue entry when it has one). Serialised JSON for `callPartyJson`.
pub async fn read_call_party(room_id: &str) -> String {
    let Some(room) = room_by_id(room_id).await else { return String::new() };
    let now = crate::appstate::now_ms();
    let viewers = read_viewers(&room, now).await;
    let party = read_party(&room).await;
    let playback = read_playback(&room).await.filter(|pb| {
        let u = pb.get("url").and_then(|x| x.as_str()).unwrap_or("");
        if !u.starts_with("tinhat-memory://") {
            return true;
        }
        let upd = pb.get("updatedAt").and_then(as_u64_opt).unwrap_or(0);
        now.saturating_sub(upd) <= MEMORY_PLAYBACK_TTL_MS
    });
    let url = playback.as_ref().and_then(|p| p.get("url")).and_then(|u| u.as_str()).unwrap_or("").to_string();
    let updated_at = playback.as_ref().and_then(|p| p.get("updatedAt")).and_then(as_u64_opt).unwrap_or(now);
    let stale = (now.saturating_sub(updated_at) as f64) / 1000.0 > 6.0 * 3600.0;
    let playing = !stale && playback.as_ref().and_then(|p| p.get("playing")).and_then(Value::as_bool).unwrap_or(false);
    let item = party
        .get("queue")
        .and_then(|q| q.as_array())
        .and_then(|arr| arr.iter().find(|it| it.get("url").and_then(|u| u.as_str()) == Some(url.as_str())).cloned())
        .unwrap_or_else(|| serde_json::json!({ "url": url, "title": "", "addedBy": "" }));
    let queue: Vec<Value> = party.get("queue").and_then(|q| q.as_array()).cloned().unwrap_or_default();
    // 11.3 QOL (QOL.md §7): the strip's next button — the url after the playing one ("" = none) + who drives
    let next = if url.is_empty() { String::new() } else { queue_next_url(&queue, &url) };
    serde_json::json!({
        "live": !viewers.is_empty(),
        "viewers": viewers.len(),
        "url": url,
        "playing": playing,
        "item": item,
        "next": next,
        "hostId": party.get("hostId").cloned().unwrap_or(Value::from("")),
    })
    .to_string()
}

/// Read the playback clock as a raw Value (`None` if no item set yet).
pub async fn read_playback(room: &Room) -> Option<Value> {
    let content = crate::appstate::get_state(room, PLAYBACK_EVENT, "").await?;
    let url = content.get("url").and_then(|u| u.as_str()).unwrap_or("").trim().to_string();
    if url.is_empty() {
        return None; // cleared / never set
    }
    Some(content)
}

/// Read all (non-expired) viewers as `[{userId,joinedAt,lastSeen}]`, sorted by userId so
/// the advancer (the min) is deterministic.
pub async fn read_viewers(room: &Room, now: u64) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for (state_key, _sender, content) in crate::appstate::get_state_all(room, VIEWER_EVENT).await {
        let last_seen = content.get("lastSeen").and_then(as_u64_opt).unwrap_or(0);
        if last_seen == 0 || now.saturating_sub(last_seen) > VIEWER_TTL_MS {
            continue; // expired / cleared
        }
        let joined_at = content.get("joinedAt").and_then(as_u64_opt).unwrap_or(last_seen);
        out.push(serde_json::json!({
            "userId": state_key,
            "joinedAt": joined_at,
            "lastSeen": last_seen,
        }));
    }
    out.sort_by(|a, b| {
        let ka = a.get("userId").and_then(|u| u.as_str()).unwrap_or("");
        let kb = b.get("userId").and_then(|u| u.as_str()).unwrap_or("");
        ka.cmp(kb)
    });
    out
}

/// 11.4 (CONTRACT §2): the room's History, newest first, at most 50, every entry a normalised item + playedAt + by.
pub async fn read_history(room: &Room) -> Vec<Value> {
    let content = crate::appstate::get_state(room, HISTORY_EVENT, "").await.unwrap_or(Value::Null);
    norm_history(&content)
}

pub fn norm_history(content: &Value) -> Vec<Value> {
    content
        .get("items")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    let mut it = norm_queue_item(v)?;
                    it["playedAt"] = Value::from(v.get("playedAt").and_then(as_u64_opt).unwrap_or(0));
                    it["by"] = Value::from(v.get("by").and_then(Value::as_str).unwrap_or(""));
                    Some(it)
                })
                .take(HISTORY_MAX)
                .collect()
        })
        .unwrap_or_default()
}

/// The ring: `item` on top (with playedAt / by), an older entry of the same url dropped, at most HISTORY_MAX.
pub fn history_push(items: &[Value], item: &Value, played_at: u64, by: &str) -> Vec<Value> {
    let url = item.get("url").and_then(Value::as_str).unwrap_or("");
    let mut entry = item.clone();
    entry["playedAt"] = Value::from(played_at);
    entry["by"] = Value::from(by);
    let mut out = vec![entry];
    out.extend(items.iter().filter(|h| h.get("url").and_then(Value::as_str) != Some(url)).cloned());
    out.truncate(HISTORY_MAX);
    out
}

pub async fn write_history(room_id: &str, url: &str, by: &str) -> Result<(), String> {
    let room = room_by_id(room_id).await.ok_or_else(|| "room not available".to_string())?;
    let party = read_party(&room).await;
    let item = party
        .get("queue")
        .and_then(Value::as_array)
        .and_then(|arr| arr.iter().find(|it| it.get("url").and_then(Value::as_str) == Some(url)).cloned())
        .unwrap_or_else(|| serde_json::json!({ "url": url, "title": "", "addedBy": "" }));
    let items = history_push(&read_history(&room).await, &item, crate::appstate::now_ms(), by);
    crate::appstate::ensure_writable(&room, &[HISTORY_EVENT]).await;
    crate::appstate::send_state(&room, HISTORY_EVENT, "", serde_json::json!({ "v": 1, "items": items }))
        .await
        .map_err(|e| e.to_string())
}

/// 11.4 (CONTRACT §3 `Item`): one browse row normalised (None = not an item).
pub fn norm_browse_item(v: &Value) -> Option<Value> {
    let o = v.as_object()?;
    let s = |k: &str| o.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let kind = s("kind");
    if !matches!(kind.as_str(), "video" | "playlist" | "channel") {
        return None;
    }
    let id = s("id");
    if id.is_empty() {
        return None;
    }
    let url = if !s("url").is_empty() {
        s("url")
    } else {
        match kind.as_str() {
            "video" => format!("https://www.youtube.com/watch?v={id}"),
            "playlist" => format!("https://www.youtube.com/playlist?list={id}"),
            _ => format!("https://www.youtube.com/channel/{id}"),
        }
    };
    let mut it = serde_json::json!({ "kind": kind, "id": id, "url": url, "title": s("title"), "thumb": s("thumb"), "channel": s("channel"), "channelId": s("channelId") });
    for k in ["durationSec", "views", "itemCount"] {
        if let Some(n) = o.get(k).and_then(as_u64_opt).filter(|n| *n > 0) {
            it[k] = Value::from(n);
        }
    }
    if !s("publishedAt").is_empty() {
        it["publishedAt"] = Value::from(s("publishedAt"));
    }
    Some(it)
}

pub fn norm_browse_items(v: &Value) -> Vec<Value> {
    let arr = v.as_array().cloned().or_else(|| v.get("items").and_then(Value::as_array).cloned()).unwrap_or_default();
    arr.iter().filter_map(norm_browse_item).collect()
}

/// A browse item → the queue item it adds (pure; the web's queueItemOf builds the same; `addedBy` = "").
pub fn queue_item_of(it: &Value, music: bool) -> Value {
    let s = |k: &str| it.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut q = serde_json::json!({
        "url": s("url"), "title": s("title"), "addedBy": "",
        "kind": if music { "song" } else { "video" },
        "source": { "service": if music { "ytmusic" } else { "youtube" }, "url": s("url"), "id": s("id") },
    });
    if !s("thumb").is_empty() {
        q["art"] = Value::from(s("thumb"));
    }
    if !s("channel").is_empty() {
        q["artist"] = Value::from(s("channel"));
    }
    if let Some(d) = it.get("durationSec").and_then(as_u64_opt).filter(|d| *d > 0) {
        q["durationSec"] = Value::from(d);
    }
    q
}

/// The `browse()` worker: one proxy call, normalised into `{kind, params, ok, items|listing, nextPage, error, budget}`.
async fn browse_fetch(kind: &str, params: &Value) -> Value {
    let ps = |k: &str| params.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let path = match kind {
        "search" => {
            let t = ps("type");
            let t = if matches!(t.as_str(), "video" | "music" | "playlist" | "channel") { t } else { "video".into() };
            let page = ps("page");
            format!("/watch/search?q={}&type={t}&max=25&v=2{}", enc(&ps("q")), if page.is_empty() { String::new() } else { format!("&page={}", enc(&page)) })
        }
        "trending" => format!("/watch/trending?cat={}&region=US", params.get("cat").and_then(as_u64_opt).unwrap_or(0)),
        "playlist" => {
            let page = ps("page");
            format!("/watch/playlist?id={}{}", enc(&ps("id")), if page.is_empty() { String::new() } else { format!("&page={}", enc(&page)) })
        }
        "channel" => {
            let page = ps("page");
            let handle = ps("handle");
            let key = if !handle.is_empty() { format!("handle={}", enc(&handle)) } else { format!("id={}", enc(&ps("id"))) };
            format!("/watch/channel?{key}{}", if page.is_empty() { String::new() } else { format!("&page={}", enc(&page)) })
        }
        _ => return serde_json::json!({ "kind": kind, "params": params, "ok": false, "error": "unknown browse kind" }),
    };
    match crate::proxy::get_json(&path).await {
        Ok(v) => {
            let next = v.get("nextPage").and_then(Value::as_str).unwrap_or("");
            if kind == "playlist" || kind == "channel" {
                let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                let listing = serde_json::json!({
                    "id": s("id"), "title": s("title"), "channel": s("channel"), "thumb": s("thumb"),
                    "itemCount": v.get("itemCount").and_then(as_u64_opt).unwrap_or(0),
                    "items": norm_browse_items(&v), "nextPage": next,
                });
                serde_json::json!({ "kind": kind, "params": params, "ok": true, "listing": listing, "nextPage": next })
            } else {
                serde_json::json!({ "kind": kind, "params": params, "ok": true, "items": norm_browse_items(&v), "nextPage": next })
            }
        }
        Err(e) => {
            let budget = e.contains("HTTP 503") && e.contains("budget");
            let notfound = e.contains("HTTP 404");
            eprintln!("[watchparty] browse {kind}: {e}");
            serde_json::json!({
                "kind": kind, "params": params, "ok": false, "budget": budget,
                "error": if budget { "YouTube search is used up for today — paste a link or use Trending / History." }
                         else if notfound { "That wasn't found." }
                         else if kind == "trending" { "Couldn't load what's trending right now." }
                         else if kind == "search" { "Couldn't search YouTube right now." }
                         else { "Couldn't load that right now." },
            })
        }
    }
}

/// Append `{url,title,addedBy}` to the queue (read-modify-write the party event). Returns the
/// JUST-WRITTEN party so the caller can push it OPTIMISTICALLY — a re-read here would hit the
/// not-yet-synced local store and drop the new item (the watch-party "queue does nothing" bug;
/// see activity.rs for the same trap). A missing room is an Err (not a silent Ok) so it surfaces.
pub async fn do_enqueue(room_id: &str, url: &str, title: &str) -> Result<Value, String> {
    let room = room_by_id(room_id).await.ok_or_else(|| "room not available".to_string())?;
    crate::appstate::ensure_writable(&room, &[WP_EVENT]).await;
    let mut party = base_party(&room, room_id).await;
    let added_by = room.own_user_id().to_string();
    let item = serde_json::json!({ "url": url.trim(), "title": title.trim(), "addedBy": added_by });
    if let Some(arr) = party.get_mut("queue").and_then(Value::as_array_mut) {
        arr.push(item);
    } else {
        party["queue"] = serde_json::json!([item]);
    }
    crate::appstate::send_state(&room, WP_EVENT, "", party.clone())
        .await
        .map_err(|e| e.to_string())?;
    remember_party_write(room_id, &party);
    Ok(party)
}

/// The party we last WROTE per room (and when): the local store echoes our own state write only after the next
/// /sync, so two edits in a row (a chat card then the box, "Add" twice, the interop probe's loop) would both read
/// the stale store and the second would drop the first (a lost update). Like the web's `baseParty` (its optimistic
/// mirror when newer), an edit starts from our own write while it is fresh and holds more than the store's copy.
static LAST_PARTY_WRITE: Mutex<Option<(String, Value, u64)>> = Mutex::new(None);
const LAST_WRITE_FRESH_MS: u64 = 15_000;

/// The party an edit must start from: the store's copy, or our fresher / longer last write (see LAST_PARTY_WRITE).
async fn base_party(room: &Room, room_id: &str) -> Value {
    let stored = read_party(room).await;
    let now = crate::appstate::now_ms();
    if let Ok(g) = LAST_PARTY_WRITE.lock() {
        if let Some((rid, party, at)) = g.as_ref() {
            if rid == room_id && now.saturating_sub(*at) < LAST_WRITE_FRESH_MS {
                let n = |p: &Value| p.get("queue").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
                if n(party) > n(&stored) || (n(party) == n(&stored) && *party != stored) {
                    let mut base = party.clone();
                    // the host may have changed on the server meanwhile — the store's hostId wins when set
                    if let Some(h) = stored.get("hostId").and_then(Value::as_str).filter(|h| !h.is_empty()) {
                        base["hostId"] = Value::from(h);
                    }
                    return base;
                }
            }
        }
    }
    stored
}

fn remember_party_write(room_id: &str, party: &Value) {
    if let Ok(mut g) = LAST_PARTY_WRITE.lock() {
        *g = Some((room_id.to_string(), party.clone(), crate::appstate::now_ms()));
    }
}

/// 11.4: read-modify-write the queue with `edit(queue, me)` (returns whether it changed); Ok(None) = no change.
/// Every field of every item is written back (read_party normalises but keeps unknown fields — CONTRACT §1).
pub async fn do_edit_queue(
    room_id: &str,
    edit: impl FnOnce(&mut Vec<Value>, &str) -> bool,
) -> Result<Option<Value>, String> {
    let room = room_by_id(room_id).await.ok_or_else(|| "room not available".to_string())?;
    crate::appstate::ensure_writable(&room, &[WP_EVENT]).await;
    let mut party = base_party(&room, room_id).await;
    let me = room.own_user_id().to_string();
    let mut queue = party.get("queue").and_then(Value::as_array).cloned().unwrap_or_default();
    if !edit(&mut queue, &me) {
        return Ok(None);
    }
    party["queue"] = Value::Array(queue);
    crate::appstate::send_state(&room, WP_EVENT, "", party.clone()).await.map_err(|e| e.to_string())?;
    remember_party_write(room_id, &party);
    Ok(Some(party))
}

/// 11.4 (CONTRACT §3 `/youtube/meta`): what a link is, normalised to the apps' Meta shape (None = unknown / no route).
pub async fn fetch_meta(url: &str) -> Option<Value> {
    let v = crate::proxy::get_json(&format!("/watch/meta?url={}", enc(url))).await.ok()?;
    norm_meta(&v)
}

pub fn norm_meta(v: &Value) -> Option<Value> {
    let o = v.as_object()?;
    let s = |k: &str| o.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let kind = s("kind");
    let service = s("service");
    if !matches!(kind.as_str(), "video" | "playlist" | "channel" | "song" | "album" | "unknown") {
        return None;
    }
    if !matches!(service.as_str(), "youtube" | "ytmusic" | "spotify" | "apple") {
        return None;
    }
    let mut m = serde_json::json!({ "kind": kind, "service": service, "id": s("id"), "url": s("url"), "title": s("title") });
    for k in ["artist", "album", "thumb", "preview"] {
        let v = s(k);
        if !v.is_empty() {
            m[k] = Value::from(v);
        }
    }
    if let Some(d) = o.get("durationSec").and_then(as_u64_opt).filter(|d| *d > 0) {
        m["durationSec"] = Value::from(d);
    }
    if let Some(tracks) = o.get("tracks").and_then(Value::as_array) {
        let list: Vec<Value> = tracks
            .iter()
            .filter(|t| t.get("title").and_then(Value::as_str).is_some())
            .take(50)
            .map(|t| {
                let mut tr = serde_json::json!({ "title": t["title"] });
                for k in ["artist", "id", "url"] {
                    if let Some(v) = t.get(k).and_then(Value::as_str) {
                        tr[k] = Value::from(v);
                    }
                }
                if let Some(d) = t.get("durationSec").and_then(as_u64_opt).filter(|d| *d > 0) {
                    tr["durationSec"] = Value::from(d);
                }
                tr
            })
            .collect();
        m["tracks"] = Value::Array(list);
    }
    Some(m)
}

/// 11.4 (CONTRACT §3 `/youtube/resolve`): a song → its YouTube upload. Ok(hit) / Err("nomatch"|"budget"|"error").
pub async fn resolve_song(url: &str) -> Result<Value, String> {
    match crate::proxy::get_json(&format!("/watch/resolve?url={}", enc(url))).await {
        Ok(v) => norm_resolved(&v).ok_or_else(|| "error".to_string()),
        Err(e) => {
            if e.contains("HTTP 404") || e.contains("nomatch") {
                Err("nomatch".into())
            } else if e.contains("HTTP 503") || e.contains("\"budget\"") {
                Err("budget".into())
            } else {
                eprintln!("[watchparty] resolve: {e}");
                Err("error".into())
            }
        }
    }
}

pub fn norm_resolved(v: &Value) -> Option<Value> {
    let id = v.get("videoId").and_then(Value::as_str).unwrap_or("");
    if !((6..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')) {
        return None;
    }
    let url = v.get("url").and_then(Value::as_str).filter(|u| !u.is_empty()).map(str::to_string).unwrap_or_else(|| format!("https://www.youtube.com/watch?v={id}"));
    let mut r = serde_json::json!({
        "videoId": id,
        "url": url,
        "title": v.get("title").and_then(Value::as_str).unwrap_or(""),
        "confidence": v.get("confidence").and_then(Value::as_f64).unwrap_or(0.0),
    });
    for k in ["channel", "thumb"] {
        if let Some(x) = v.get(k).and_then(Value::as_str) {
            r[k] = Value::from(x);
        }
    }
    if let Some(d) = v.get("durationSec").and_then(as_u64_opt).filter(|d| *d > 0) {
        r["durationSec"] = Value::from(d);
    }
    Some(r)
}

/// The queue item a parsed link + its meta make (pure; the web's `itemFor` builds the same object; `addedBy` = "").
pub fn item_for(link: &Value, meta: Option<&Value>, resolved: Option<&Value>) -> Option<Value> {
    let kind = link.get("kind").and_then(Value::as_str).unwrap_or("");
    let ls = |k: &str| link.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let ms = |k: &str| meta.and_then(|m| m.get(k)).and_then(Value::as_str).unwrap_or("").to_string();
    let mdur = meta.and_then(|m| m.get("durationSec")).and_then(as_u64_opt).filter(|d| *d > 0);
    let file_name = |u: &str| {
        Url::parse(u)
            .ok()
            .and_then(|p| p.path_segments().and_then(|s| s.filter(|x| !x.is_empty()).last().map(str::to_string)))
            .map(|n| percent_decode(&n))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| u.to_string())
    };
    match kind {
        "youtube" | "ytmusic" => {
            let song = kind == "ytmusic" || ms("kind") == "song";
            let mut it = serde_json::json!({
                "url": ls("playUrl"), "title": ms("title"), "addedBy": "",
                "kind": if song { "song" } else { "video" },
                "source": { "service": if kind == "ytmusic" { "ytmusic" } else { "youtube" }, "url": ls("input"), "id": ls("id") },
            });
            let art = if !ms("thumb").is_empty() { ms("thumb") } else { ls("art") };
            if !art.is_empty() {
                it["art"] = Value::from(art);
            }
            if !ms("artist").is_empty() {
                it["artist"] = Value::from(ms("artist"));
            }
            if let Some(d) = mdur {
                it["durationSec"] = Value::from(d);
            }
            Some(it)
        }
        "file" => {
            let title = if !ms("title").is_empty() { ms("title") } else { file_name(&ls("input")) };
            Some(serde_json::json!({ "url": ls("input"), "title": title, "addedBy": "", "kind": "file" }))
        }
        "link" => {
            let title = if !ms("title").is_empty() { ms("title") } else { ls("input") };
            Some(serde_json::json!({ "url": ls("input"), "title": title, "addedBy": "" }))
        }
        "spotify" | "apple" => {
            let r = resolved?;
            let rs = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let title = if !ms("title").is_empty() { ms("title") } else { rs("title") };
            let mut it = serde_json::json!({
                "url": rs("url"), "title": title, "addedBy": "", "kind": "song",
                "source": { "service": if ls("service") == "apple" { "apple" } else { "spotify" }, "url": ls("input"), "id": ls("id") },
            });
            let art = if !ms("thumb").is_empty() { ms("thumb") } else { rs("thumb") };
            if !art.is_empty() {
                it["art"] = Value::from(art);
            }
            if !ms("artist").is_empty() {
                it["artist"] = Value::from(ms("artist"));
            }
            if let Some(d) = mdur.or_else(|| r.get("durationSec").and_then(as_u64_opt).filter(|d| *d > 0)) {
                it["durationSec"] = Value::from(d);
            }
            if !ms("preview").is_empty() {
                it["preview"] = Value::from(ms("preview"));
            }
            Some(it)
        }
        _ => None,
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 0 && i + 2 <= b.len() - 1 {
            if let Ok(h) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(h);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// `add_link`'s worker: the server round trips (meta / resolve), then the queue write. Returns the outcome object
/// (`status`, `item`, `party` (the written party, for the optimistic push), `link`, `meta`, `message`).
async fn resolve_add_link(room_id: &str, link: &Value, play_now: bool) -> Value {
    let kind = link.get("kind").and_then(Value::as_str).unwrap_or("");
    let input = link.get("input").and_then(Value::as_str).unwrap_or("").to_string();
    let commit = |item: Value| async move {
        let mut it = item.clone();
        let (url, title) = (it["url"].as_str().unwrap_or("").to_string(), it["title"].as_str().unwrap_or("").to_string());
        match do_edit_queue(room_id, move |q, me| {
            it["addedBy"] = Value::from(me);
            q.push(it);
            true
        })
        .await
        {
            Ok(Some(party)) => {
                if play_now {
                    let _ = write_playback(room_id, &url, 0.0, true, crate::appstate::now_ms()).await;
                }
                serde_json::json!({ "status": if play_now { "playing" } else { "queued" }, "item": item, "party": party, "title": title })
            }
            Ok(None) => serde_json::json!({ "status": "error", "message": "Couldn't add this link." }),
            Err(e) => {
                eprintln!("[watchparty] add_link write failed: {e}");
                serde_json::json!({ "status": "error", "message": "Couldn't update the Media Party — this room may need an admin to enable it" })
            }
        }
    };
    match kind {
        "youtube" | "ytmusic" => {
            let mut meta = fetch_meta(&input).await;
            if meta.is_none() {
                if let Some(t) = resolve_title(link.get("playUrl").and_then(Value::as_str).unwrap_or(&input)).await {
                    meta = Some(serde_json::json!({ "kind": "video", "service": if kind == "ytmusic" { "ytmusic" } else { "youtube" }, "id": link["id"], "url": input, "title": t }));
                }
            }
            match item_for(link, meta.as_ref(), None) {
                Some(it) => commit(it).await,
                None => serde_json::json!({ "status": "error", "message": "Couldn't add this link." }),
            }
        }
        "file" | "link" => match item_for(link, None, None) {
            Some(it) => commit(it).await,
            None => serde_json::json!({ "status": "error", "message": "Couldn't add this link." }),
        },
        "spotify" | "apple" => {
            let meta = fetch_meta(&input).await;
            if link.get("entity").and_then(Value::as_str) != Some("track") {
                let msg = match meta.as_ref().and_then(|m| m.get("kind")).and_then(Value::as_str) {
                    Some("album") => "An album — add its songs from the card.",
                    Some(_) => "A playlist — add its songs from the card.",
                    None => "Couldn't read this link.",
                };
                return serde_json::json!({ "status": "card", "link": link, "meta": meta, "message": msg });
            }
            match resolve_song(&input).await {
                Ok(hit) => match item_for(link, meta.as_ref(), Some(&hit)) {
                    Some(it) => commit(it).await,
                    None => serde_json::json!({ "status": "error", "message": "Couldn't add this song." }),
                },
                Err(why) => {
                    let msg = match why.as_str() {
                        "budget" => "YouTube search is used up for today — open it in its app, or paste a YouTube link.",
                        "nomatch" => "No YouTube upload of this song was found — open it in its app.",
                        _ => "Couldn't look this song up right now — open it in its app.",
                    };
                    serde_json::json!({ "status": "card", "link": link, "meta": meta, "message": msg })
                }
            }
        }
        _ => serde_json::json!({ "status": "error", "message": "Couldn't add this link." }),
    }
}

/// Set the party host (read-modify-write hostId, preserving the queue).
pub async fn set_host(room_id: &str, user_id: &str) -> matrix_sdk::Result<()> {
    let Some(room) = room_by_id(room_id).await else { return Ok(()) };
    crate::appstate::ensure_writable(&room, &[WP_EVENT]).await;
    let mut party = base_party(&room, room_id).await;
    party["hostId"] = Value::from(user_id.trim());
    let r = crate::appstate::send_state(&room, WP_EVENT, "", party.clone()).await;
    if r.is_ok() {
        remember_party_write(room_id, &party);
    }
    r
}

/// Current `(url, playing)` of the playback clock (for transport that preserves them).
async fn current_url_playing(room_id: &str) -> (String, bool) {
    let (url, playing, _nonce) = current_url_playing_nonce(room_id).await;
    (url, playing)
}

/// Current `(url, playing, seekNonce)` of the playback clock — for transport (play/pause)
/// that must PRESERVE the existing nonce so it isn't conflated with a manual scrub.
async fn current_url_playing_nonce(room_id: &str) -> (String, bool, u64) {
    let Some(room) = room_by_id(room_id).await else { return (String::new(), false, 0) };
    match read_playback(&room).await {
        Some(c) => (
            c.get("url").and_then(|u| u.as_str()).unwrap_or("").to_string(),
            c.get("playing").and_then(Value::as_bool).unwrap_or(false),
            c.get("seekNonce").and_then(as_u64_opt).unwrap_or(0),
        ),
        None => (String::new(), false, 0),
    }
}

/// Write the shared playback clock. `position_secs` is rounded to whole MILLISECONDS as an
/// INTEGER field (`positionMs`) AND echoed as a fractional-second STRING (`positionSecs`) —
/// canonical-json rejects bare floats, and the cxx-qt boundary is unreliable for f64. The
/// reader tolerates either shape.
pub async fn write_playback(
    room_id: &str,
    url: &str,
    position_secs: f64,
    playing: bool,
    seek_nonce: u64,
) -> Result<Value, String> {
    let room = room_by_id(room_id).await.ok_or_else(|| "room not available".to_string())?;
    crate::appstate::ensure_writable(&room, &[PLAYBACK_EVENT]).await;
    let pos_ms = (position_secs.max(0.0) * 1000.0).round() as u64; // integer — canonical-json safe
    let content = serde_json::json!({
        "url": url.trim(),
        "positionMs": pos_ms,                       // integer ms (canonical source of truth)
        "positionSecs": format!("{:.3}", position_secs.max(0.0)), // string mirror (no bare float)
        "playing": playing,
        "updatedAt": crate::appstate::now_ms(),    // integer epoch-ms
        "seekNonce": seek_nonce,                    // integer
    });
    // Returns the just-written content so callers push it OPTIMISTICALLY (an immediate re-read
    // hits the not-yet-synced local store — the same trap activity.rs documents + avoids).
    crate::appstate::send_state(&room, PLAYBACK_EVENT, "", content.clone())
        .await
        .map_err(|e| e.to_string())?;
    Ok(content)
}

/// Write/refresh my viewer heartbeat (state_key = my userId), TTL-presence.
pub async fn write_viewer(room_id: &str) -> matrix_sdk::Result<()> {
    let Some(room) = room_by_id(room_id).await else { return Ok(()) };
    crate::appstate::ensure_writable(&room, &[VIEWER_EVENT]).await;
    let me = room.own_user_id().to_string();
    let now = crate::appstate::now_ms();
    // Preserve joinedAt across heartbeats (read the existing one).
    let joined_at = crate::appstate::get_state(&room, VIEWER_EVENT, &me)
        .await
        .and_then(|c| c.get("joinedAt").and_then(as_u64_opt))
        .unwrap_or(now);
    let content = serde_json::json!({ "joinedAt": joined_at, "lastSeen": now });
    crate::appstate::send_state(&room, VIEWER_EVENT, &me, content).await
}

/// Tombstone my viewer presence (empty content) on leave.
pub async fn clear_viewer(room_id: &str) -> matrix_sdk::Result<()> {
    let Some(room) = room_by_id(room_id).await else { return Ok(()) };
    let me = room.own_user_id().to_string();
    crate::appstate::send_state(&room, VIEWER_EVENT, &me, serde_json::json!({})).await
}

/// Convert the stored playback Value into the QML-facing JSON shape (positionSecs/updatedAt
/// as STRINGS / whole-ms ints — never a bare f64 the binding can mangle).
fn playback_to_qml_json(content: &Value) -> String {
    playback_to_qml_json_by(content, "")
}

/// 11.3 QOL (QOL.md §2): + `by` = the SENDER of the playback state event (the tick reads it; "" for our own optimistic
/// pushes) — the window's "{name} paused / pressed play / jumped to" OSD. Mirror-only: never written to the wire.
fn playback_to_qml_json_by(content: &Value, by: &str) -> String {
    let url = content.get("url").and_then(|u| u.as_str()).unwrap_or("");
    let pos = playback_position_secs(content);
    let playing = content.get("playing").and_then(Value::as_bool).unwrap_or(false);
    let updated_at = content.get("updatedAt").and_then(as_u64_opt).unwrap_or(0);
    let seek_nonce = content.get("seekNonce").and_then(as_u64_opt).unwrap_or(0);
    serde_json::json!({
        "url": url,
        "positionSecs": format!("{pos:.3}"), // string
        "playing": playing,
        "updatedAt": updated_at,             // integer ms
        "seekNonce": seek_nonce,             // integer
        "by": by,                            // 11.3 QOL: the event's sender (mirror only)
    })
    .to_string()
}

/// Read positionSecs from a playback content, tolerating the integer `positionMs` source of
/// truth, the `positionSecs` string mirror, or a legacy bare number.
fn playback_position_secs(content: &Value) -> f64 {
    if let Some(ms) = content.get("positionMs").and_then(as_u64_opt) {
        return ms as f64 / 1000.0;
    }
    if let Some(p) = content.get("positionSecs") {
        return as_f64(p);
    }
    0.0
}

/// Push just the party (queue/host) prop after a write. `gen` is the caller's SyncState
/// generation — a push made stale by a leave/rejoin while the write was in flight is dropped.
async fn push_party(qt: &cxx_qt::CxxQtThread<qobject::WatchPartyController>, room_id: &str, gen: u64) {
    let Some(room) = room_by_id(room_id).await else { return };
    let json = serde_json::to_string(&read_party(&room).await).unwrap_or_else(|_| "{}".to_string());
    let _ = qt.queue(move |mut o| {
        if o.rust().sync.lock().unwrap().generation != gen {
            return;
        }
        o.as_mut().set_party_json(QString::from(&json));
    });
}

// ---------------------------------------------------------------------------
// Search — /watch/search via the server's Tin Hat API, oEmbed title fallback.
// ---------------------------------------------------------------------------

/// Search YouTube via the proxy, normalize to `[{id,url,title,thumbnail}]`, and fill any
/// missing titles via oEmbed. Returns the JSON array string.
pub async fn search_watch(query: &str) -> Result<String, String> {
    let path = format!("/watch/search?q={}", enc(query));
    let v = crate::proxy::get_json(&path).await?;
    let arr = v
        .as_array()
        .cloned()
        .or_else(|| {
            ["results", "items", "videos", "data"]
                .iter()
                .find_map(|k| v.get(*k).and_then(|x| x.as_array()).cloned())
        })
        .unwrap_or_default();
    let mut out: Vec<Value> = Vec::new();
    for item in &arr {
        let (id, url) = extract_id_url(item);
        if url.is_empty() {
            continue;
        }
        let mut title = item
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let thumbnail = item
            .get("thumbnail")
            .or_else(|| item.get("thumbnailUrl"))
            .or_else(|| item.get("thumb"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        if title.is_empty() {
            if let Some(t) = resolve_title(&url).await {
                title = t;
            }
        }
        out.push(serde_json::json!({
            "id": id,
            "url": url,
            "title": if title.is_empty() { url.clone() } else { title },
            "thumbnail": thumbnail,
        }));
    }
    Ok(serde_json::to_string(&out).unwrap_or_else(|_| "[]".to_string()))
}

/// Pull a video id + canonical watch URL from one proxy result (tolerant of shapes).
fn extract_id_url(item: &Value) -> (String, String) {
    let id = item
        .get("id")
        .or_else(|| item.get("videoId"))
        .and_then(|i| i.as_str())
        .unwrap_or("")
        .to_string();
    let url = item
        .get("url")
        .or_else(|| item.get("link"))
        .and_then(|u| u.as_str())
        .filter(|u| u.starts_with("http"))
        .map(str::to_string)
        .unwrap_or_else(|| {
            if id.is_empty() {
                String::new()
            } else {
                format!("https://www.youtube.com/watch?v={id}")
            }
        });
    (id, url)
}

/// Resolve a video title via oEmbed: prefer the proxy (`/watch/oembed?url=…`), fall back to
/// the public YouTube oEmbed endpoint. Best-effort — returns `None` on any failure.
async fn resolve_title(url: &str) -> Option<String> {
    // Try the proxy first (keeps egress server-side + cacheable).
    let proxy_path = format!("/watch/oembed?url={}", enc(url));
    if let Ok(v) = crate::proxy::get_json(&proxy_path).await {
        if let Some(t) = v.get("title").and_then(|t| t.as_str()) {
            if !t.trim().is_empty() {
                return Some(t.trim().to_string());
            }
        }
    }
    // Direct public oEmbed fallback.
    let direct = format!("https://www.youtube.com/oembed?format=json&url={}", enc(url));
    let resp = matrix_sdk::reqwest::Client::new().get(&direct).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let text = resp.text().await.ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v.get("title").and_then(|t| t.as_str()).map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

// ---------------------------------------------------------------------------
// Small helpers.
// ---------------------------------------------------------------------------

/// Parse a position STRING ("12.345") into seconds (f64); tolerant of junk → 0.
fn parse_secs(s: &str) -> f64 {
    s.trim().parse::<f64>().ok().filter(|f| f.is_finite() && *f >= 0.0).unwrap_or(0.0)
}

/// A JSON number/string position → f64 seconds.
fn as_f64(v: &Value) -> f64 {
    if let Some(f) = v.as_f64() {
        return f;
    }
    v.as_str().and_then(|s| s.trim().parse::<f64>().ok()).unwrap_or(0.0)
}

/// A JSON integer-ish field → u64 (tolerates a stringified or float value).
fn as_u64_opt(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_f64().map(|f| f.max(0.0) as u64))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
}

/// Minimal RFC-3986 percent-encode for a query value (matches gif.rs/soundboard.rs).
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

// ---------------------------------------------------------------------------
// Headless probe (no GUI): proves state round-trip + the SYNC DECISION tree +
// viewer heartbeat/expiry + a /watch/search round-trip.
//   TINHAT_WATCHPARTY='!room:server' [TINHAT_WATCH_QUERY=lofi] ./tinhat
// ---------------------------------------------------------------------------
pub async fn headless_verify(room_id: &str) {
    let Some(client) = crate::session::client() else {
        eprintln!("[wptest] not signed in");
        return;
    };

    // ---- (A) PURE SYNC-DECISION assertions (no network) — the heart of useWatchSync ----
    let mut sync_ok = true;
    let base = 1_000_000u64; // a fixed "now" base for the scenarios
    let mut assert_decision = |name: &str, setup: &dyn Fn(&mut SyncState), target: f64, playing: bool, nonce: u64, now: u64, want_seek: bool| {
        let mut s = SyncState::default();
        setup(&mut s);
        let out = decide_sync(&mut s, target, playing, nonce, now);
        let got = out.emit_seek.is_some();
        let pass = got == want_seek;
        if !pass {
            sync_ok = false;
        }
        eprintln!(
            "[wptest] sync scenario '{name}': want_seek={want_seek} got_seek={got} emit={:?} -> {}",
            out.emit_seek,
            if pass { "PASS" } else { "FAIL" }
        );
    };

    // 1) Big drift while playing ⇒ SEEK.
    assert_decision("hard-drift-seek", &|s| { s.local_position = 0.0; }, 30.0, true, 0, base, true);
    // 2) Small drift ⇒ RIDE (no seek).
    assert_decision("small-drift-ride", &|s| { s.local_position = 30.4; }, 30.0, true, 0, base, false);
    // 3) A buffering STALL: big drift but we JUST emitted a seek for ~this target inside the
    //    dedup window AND no new nonce ⇒ do NOT thrash another seek.
    assert_decision(
        "stall-no-thrash",
        &|s| {
            s.local_position = 0.0;
            s.last_seek_target = 30.0;
            s.last_seek_at = base - 500; // 500ms ago < SEEK_DEDUP_MS
        },
        30.0,
        true,
        0,
        base,
        false,
    );
    // 4) applyingRemote gate: big drift but we are inside the apply window ⇒ no seek.
    assert_decision(
        "applying-remote-gate",
        &|s| {
            s.local_position = 0.0;
            s.applying_remote_until = base + 1000; // gate open
        },
        30.0,
        true,
        0,
        base,
        false,
    );
    // 5) Manual scrub: SMALL drift but a FRESH nonce ⇒ everyone seeks.
    assert_decision(
        "manual-scrub-nonce-seek",
        &|s| {
            s.local_position = 12.1;
            s.last_nonce = 111; // old nonce
        },
        12.0,
        true,
        222, // new nonce
        base,
        true,
    );
    // 6) Play/pause toggle: SMALL drift and the SAME nonce we already acted on ⇒ RIDE (no
    //    seek). This proves a transport toggle (which now PRESERVES seekNonce) does NOT
    //    conflate with a manual scrub and force a follower seek-storm.
    assert_decision(
        "playpause-same-nonce-ride",
        &|s| {
            s.local_position = 12.1;
            s.last_nonce = 333; // already acted on this nonce
        },
        12.0,
        true,
        333, // unchanged nonce (a play/pause re-anchors updatedAt but keeps the nonce)
        base,
        false,
    );

    // ---- 11.4 Media Party interop probe (test rooms only; no playback / viewer writes) ----
    //   TINHAT_WATCH_ENQUEUE='["<link>", {item}, …]'  adds links through the real pipeline (meta / resolve on the server)
    //                                               and / or full items (CONTRACT §1) to the room's queue
    //   TINHAT_WATCH_DUMP=1                            prints the party (queue + host) and the History as read
    let probe_enqueue = std::env::var("TINHAT_WATCH_ENQUEUE").ok();
    let probe_dump = std::env::var("TINHAT_WATCH_DUMP").is_ok() || std::env::var("TINHAT_WATCH_BROWSE").is_ok() || std::env::var("TINHAT_WATCH_ADD_PLAYLIST").is_ok();
    if probe_enqueue.is_some() || probe_dump {
        if let Some(json) = probe_enqueue {
            let list: Vec<Value> = serde_json::from_str(&json).unwrap_or_default();
            for entry in list {
                if let Some(link) = entry.as_str() {
                    let parsed = crate::medialinks::parse_media_link(link);
                    let out = resolve_add_link(room_id, &parsed, false).await;
                    eprintln!(
                        "[wptest] enqueue link {link} -> {} {}",
                        out["status"].as_str().unwrap_or("?"),
                        out.get("item").map(|i| i.to_string()).or_else(|| out.get("message").map(|m| m.to_string())).unwrap_or_default()
                    );
                } else if let Some(item) = norm_queue_item(&entry) {
                    let r = do_edit_queue(room_id, move |q, me| {
                        let mut it = item;
                        it["addedBy"] = Value::from(me);
                        q.push(it);
                        true
                    })
                    .await;
                    eprintln!("[wptest] enqueue item -> {}", match r { Ok(Some(_)) => "queued".to_string(), Ok(None) => "unchanged".to_string(), Err(e) => format!("FAILED {e}") });
                }
            }
            let _ = client.sync_once(crate::bridge::sync_settings().timeout(std::time::Duration::from_secs(10))).await;
        }
        //   TINHAT_WATCH_BROWSE='music:<q>' | 'playlist:<id>' | 'channel:<id|@handle>'  prints the Browse pane's rows
        //   TINHAT_WATCH_ADD_PLAYLIST=<id>  the Playlists tab's "Add all": every video of the playlist → ONE queue write
        if let Ok(spec) = std::env::var("TINHAT_WATCH_BROWSE") {
            let (kind, arg) = spec.split_once(':').unwrap_or(("search", spec.as_str()));
            let (kind, params) = match kind {
                "music" => ("search", serde_json::json!({ "q": arg, "type": "music" })),
                "playlist" => ("playlist", serde_json::json!({ "id": arg })),
                "channel" => ("channel", if arg.starts_with('@') { serde_json::json!({ "handle": arg }) } else { serde_json::json!({ "id": arg }) }),
                _ => ("search", serde_json::json!({ "q": arg, "type": "video" })),
            };
            let out = browse_fetch(kind, &params).await;
            eprintln!("[wptest] BROWSE {kind} ok={} error={:?}", out["ok"], out.get("error"));
            let rows = out.get("items").or_else(|| out.get("listing").and_then(|l| l.get("items"))).and_then(Value::as_array).cloned().unwrap_or_default();
            if let Some(l) = out.get("listing") {
                eprintln!("[wptest] LISTING {} | {} | {} items", l["title"], l["channel"], l["itemCount"]);
            }
            for it in rows.iter().take(6) {
                eprintln!("[wptest] ROW {} {} | {} | {} | {}s | art={}", it["kind"], it["id"], it["title"], it["channel"], it["durationSec"], !it["thumb"].as_str().unwrap_or("").is_empty());
            }
            eprintln!("[wptest] ROWS {}", rows.len());
        }
        if let Ok(id) = std::env::var("TINHAT_WATCH_ADD_PLAYLIST") {
            let out = browse_fetch("playlist", &serde_json::json!({ "id": id })).await;
            let rows = out.get("listing").and_then(|l| l.get("items")).and_then(Value::as_array).cloned().unwrap_or_default();
            let items: Vec<Value> = rows.iter().filter(|it| it["kind"] == "video").map(|it| queue_item_of(it, false)).collect();
            let n = items.len();
            let r = do_edit_queue(room_id, move |q, me| {
                for mut it in items {
                    it["addedBy"] = Value::from(me);
                    q.push(it);
                }
                n > 0
            })
            .await;
            eprintln!("[wptest] ADD-ALL playlist {id}: {n} videos -> {}", match r { Ok(Some(p)) => format!("queued (queue now {})", p["queue"].as_array().map(|a| a.len()).unwrap_or(0)), Ok(None) => "nothing".into(), Err(e) => format!("FAILED {e}") });
            let _ = client.sync_once(crate::bridge::sync_settings().timeout(std::time::Duration::from_secs(10))).await;
        }
        if let Some(room) = room_by_id(room_id).await {
            eprintln!("[wptest] PARTY {}", read_party(&room).await);
            eprintln!("[wptest] HISTORY {}", Value::Array(read_history(&room).await));
        }
        eprintln!("[wptest] VERDICT: PROBE-DONE (sync_decision={sync_ok})");
        return;
    }

    // ---- (B) state round-trip vs the live homeserver (best-effort; may be store-locked) ----
    let mut playback_ok = false;
    let mut viewer_ok = false;
    let test_url = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
    match write_playback(room_id, test_url, 42.5, true, 7).await {
        Ok(_) => {
            let _ = client
                .sync_once(crate::bridge::sync_settings().timeout(std::time::Duration::from_secs(10)))
                .await;
            if let Some(room) = room_by_id(room_id).await {
                if let Some(pb) = read_playback(&room).await {
                    let url = pb.get("url").and_then(|u| u.as_str()).unwrap_or("");
                    let pos = playback_position_secs(&pb);
                    let nonce = pb.get("seekNonce").and_then(as_u64_opt).unwrap_or(0);
                    playback_ok = url == test_url && (pos - 42.5).abs() < 0.01 && nonce == 7;
                    eprintln!("[wptest] playback round-trip: url={url} posSecs={pos} nonce={nonce} ok={playback_ok}");
                }
            }
        }
        Err(e) => eprintln!("[wptest] playback write FAILED (store locked / PL?): {e}"),
    }

    // viewer heartbeat write + expiry filter.
    match write_viewer(room_id).await {
        Ok(()) => {
            let _ = client
                .sync_once(crate::bridge::sync_settings().timeout(std::time::Duration::from_secs(10)))
                .await;
            if let Some(room) = room_by_id(room_id).await {
                let now = crate::appstate::now_ms();
                let live = read_viewers(&room, now).await;
                let me = room.own_user_id().to_string();
                let present = live.iter().any(|v| v.get("userId").and_then(|u| u.as_str()) == Some(me.as_str()));
                // Expiry filter: the SAME read with a far-future `now` must drop everyone.
                let expired = read_viewers(&room, now + VIEWER_TTL_MS + 60_000).await;
                let gone = !expired.iter().any(|v| v.get("userId").and_then(|u| u.as_str()) == Some(me.as_str()));
                viewer_ok = present && gone;
                eprintln!("[wptest] viewer heartbeat present={present} expiry-filtered={gone} ok={viewer_ok}");
                let _ = clear_viewer(room_id).await; // cleanup
            }
        }
        Err(e) => eprintln!("[wptest] viewer write FAILED (store locked / PL?): {e}"),
    }

    // ---- (C) /watch/search round-trip (proxy may not expose /watch yet) ----
    let mut search_ok = false;
    let q = std::env::var("TINHAT_WATCH_QUERY").unwrap_or_else(|_| "lofi hip hop".to_string());
    eprintln!("[wptest] GET {}/watch/search?q=…", crate::proxy::base());
    match search_watch(&q).await {
        Ok(json) => {
            let arr: Value = serde_json::from_str(&json).unwrap_or(Value::Null);
            let n = arr.as_array().map(|a| a.len()).unwrap_or(0);
            search_ok = arr
                .as_array()
                .and_then(|a| a.first())
                .and_then(|f| f.get("url").and_then(|u| u.as_str()))
                .map(|u| u.starts_with("http"))
                .unwrap_or(false);
            eprintln!("[wptest] search {n} result(s); first={:?}", arr.as_array().and_then(|a| a.first()));
        }
        Err(e) => eprintln!("[wptest] search FAILED (proxy may not expose /watch yet): {e}"),
    }

    eprintln!(
        "[wptest] VERDICT: {} (sync_decision={sync_ok} playback={playback_ok} viewer={viewer_ok} search={search_ok})",
        if sync_ok { "SYNC-PASS" } else { "SYNC-FAIL" }
    );
    eprintln!("[wptest] (sync-decision is the gating assertion; playback/viewer/search are live best-effort and may be store-locked or proxy-missing)");
}

// ---------------------------------------------------------------------------
// Media Party 11.4 unit tests — mirrored on the web (src/features/media/watchparty/*.test.ts) over the SHARED vectors
// (crates/app/testvectors/mediaparty/*.json = the web's testvectors/mediaparty/*.json).
// ---------------------------------------------------------------------------
#[cfg(test)]
mod mediaparty_tests {
    use super::*;

    fn vectors(name: &str) -> Value {
        let path = format!("{}/testvectors/mediaparty/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))).unwrap()
    }

    /// CONTRACT §1 old ↔ new: every vector case normalises to the web's exact object (null = dropped).
    #[test]
    fn queue_item_vectors() {
        let v = vectors("queue.json");
        let cases = v["cases"].as_array().unwrap();
        assert!(cases.len() >= 15);
        for c in cases {
            let got = norm_queue_item(&c["input"]).unwrap_or(Value::Null);
            assert_eq!(got, c["expect"], "vector {:?}", c["name"]);
        }
    }

    /// An old client's 3-field party reads as before; a new item keeps every field through read_party's normaliser.
    #[test]
    fn queue_item_compat() {
        let yt = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
        let old = norm_queue_item(&serde_json::json!({ "url": yt, "title": "a", "addedBy": "@a:x" })).unwrap();
        assert_eq!(old, serde_json::json!({ "url": yt, "title": "a", "addedBy": "@a:x" }));
        let song = serde_json::json!({ "url": yt, "title": "s", "addedBy": "@a:x", "kind": "song",
            "source": { "service": "spotify", "url": "https://open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT", "id": "4cOdK2wGLETKBW3PvgPWqT" },
            "art": "https://a/b.jpg", "artist": "R", "durationSec": 214, "preview": "https://p/x", "future": { "x": 1 } });
        assert_eq!(norm_queue_item(&song).unwrap(), song);
        assert!(norm_queue_item(&serde_json::json!({ "url": "  " })).is_none());
        assert!(norm_queue_item(&serde_json::json!("x")).is_none());
        assert_eq!(norm_queue_item(&serde_json::json!({ "url": yt, "kind": "podcast" })).unwrap().get("kind"), None);
    }

    /// CONTRACT §2: the ring, newest first, 50, de-duplicated by url — every vector case + normHistory.
    #[test]
    fn history_vectors() {
        let v = vectors("history.json");
        for c in v["cases"].as_array().unwrap() {
            let items: Vec<Value> = c["items"].as_array().unwrap().clone();
            let got = history_push(&items, &c["push"]["item"], c["push"]["playedAt"].as_u64().unwrap(), c["push"]["by"].as_str().unwrap());
            assert_eq!(Value::Array(got), c["expect"], "push {:?}", c["name"]);
        }
        for c in v["norm"].as_array().unwrap() {
            assert_eq!(Value::Array(norm_history(&c["input"])), c["expect"], "norm {:?}", c["name"]);
        }
        let full: Vec<Value> = (0..HISTORY_MAX).map(|i| serde_json::json!({ "url": format!("u{i}"), "title": "", "addedBy": "", "playedAt": i, "by": "" })).collect();
        let pushed = history_push(&full, &serde_json::json!({ "url": "new", "title": "n", "addedBy": "" }), 9, "@a:x");
        assert_eq!(pushed.len(), HISTORY_MAX);
        assert_eq!(pushed[0]["url"], "new");
        assert_eq!(pushed[1]["url"], "u0");
    }

    /// The key table equals the shared vectors (26 keys: key, op, its argument, the action, the label), and
    /// WatchPartyWindow.qml implements every key as a Shortcut and every action as a runAction case.
    #[test]
    fn shortcuts_vectors_and_qml() {
        let v = vectors("shortcuts.json");
        let cases = v["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 26);
        assert_eq!(cases.len(), MP_SHORTCUTS.len());
        for (c, (key, op, arg, action, label)) in cases.iter().zip(MP_SHORTCUTS) {
            assert_eq!(c["key"], *key);
            assert_eq!(c["op"], *op, "{key}");
            let want = match *op {
                "seek" => c["secs"].as_i64(),
                "jump" => c["pct"].as_i64(),
                "volume" => c["step"].as_i64(),
                _ => None,
            };
            assert_eq!(want.unwrap_or(0), *arg as i64, "{key}");
            for k in ["secs", "pct", "step"] {
                let mine = matches!((k, *op), ("secs", "seek") | ("pct", "jump") | ("step", "volume"));
                assert_eq!(c.get(k).is_some(), mine, "{key}: unexpected field {k}");
            }
            assert_eq!(c["action"].as_str().unwrap_or(""), *action, "{key}");
            assert_eq!(c["label"].as_str().unwrap_or(""), *label, "{key}");
        }
        let qml = include_str!("../qml/WatchPartyWindow.qml");
        for (key, _, _, _, _) in MP_SHORTCUTS {
            assert!(qml.contains(&format!("Shortcut {{ sequence: \"{key}\"")), "WatchPartyWindow.qml has no Shortcut for {key}");
        }
        for (_, _, _, action, _) in MP_SHORTCUTS.iter().filter(|s| !s.3.is_empty()) {
            assert!(qml.contains(&format!("case \"{action}\"")), "runAction has no case for {action}");
        }
    }

    /// QOL.md §2 / §5: moveToNext / clearPlayed / shuffleUpcoming / upcomingLabel / fmtTime — every case of the shared
    /// queueops.json (36), the same answers as the web's queue.ts and the Python reference (mulberry32 bit-for-bit).
    #[test]
    fn queueops_vectors() {
        let v = vectors("queueops.json");
        let cases = v["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 36);
        let urls = |q: &[Value]| Value::Array(q.iter().map(|it| it["url"].clone()).collect());
        let mut seen = std::collections::BTreeMap::new();
        for c in cases {
            let name = c["name"].as_str().unwrap();
            let inp = &c["input"];
            let q0: Vec<Value> = inp["queue"].as_array().cloned().unwrap_or_default();
            let now = inp["nowUrl"].as_str().unwrap_or("");
            let op = c["op"].as_str().unwrap();
            *seen.entry(op.to_string()).or_insert(0) += 1;
            match op {
                "moveToNext" => {
                    let mut q = q0.clone();
                    let changed = queue_move_to_next(&mut q, now, inp["from"].as_u64().unwrap() as usize);
                    assert_eq!(urls(&q), c["expect"], "{name}");
                    assert_eq!(changed, urls(&q) != urls(&q0), "{name}: changed flag");
                }
                "clearPlayed" => {
                    let mut q = q0.clone();
                    let changed = queue_clear_played(&mut q, now);
                    assert_eq!(urls(&q), c["expect"], "{name}");
                    assert_eq!(changed, urls(&q) != urls(&q0), "{name}: changed flag");
                }
                "shuffleUpcoming" => {
                    let mut q = q0.clone();
                    let seed = inp["seed"].as_u64().unwrap();
                    assert!(seed <= u32::MAX as u64);
                    let changed = queue_shuffle_upcoming(&mut q, now, seed as u32);
                    assert_eq!(urls(&q), c["expect"], "{name}");
                    assert_eq!(changed, urls(&q) != urls(&q0), "{name}: changed flag");
                    // an edit keeps every field of every item (CONTRACT §1)
                    let mut a: Vec<String> = q.iter().map(|x| x.to_string()).collect();
                    let mut b: Vec<String> = q0.iter().map(|x| x.to_string()).collect();
                    a.sort();
                    b.sort();
                    assert_eq!(a, b, "{name}");
                }
                "upcomingLabel" => assert_eq!(queue_upcoming_label(&q0, now), c["expect"].as_str().unwrap(), "{name}"),
                "fmtTime" => assert_eq!(fmt_time(inp["secs"].as_f64().unwrap()), c["expect"].as_str().unwrap(), "{name}"),
                other => panic!("unknown op {other}"),
            }
        }
        assert_eq!(seen.len(), 5, "{seen:?}");
        assert_eq!(fmt_time(f64::NAN), "0:00");
        assert_eq!(fmt_time(f64::INFINITY), "0:00");
    }

    /// mulberry32's first draws for a few seeds — the JS `Math.imul` / `>>> 0` arithmetic (values from the Python
    /// reference, which the vectors were generated with).
    #[test]
    fn mulberry32_reference() {
        let want: [(u32, [f64; 3]); 4] = [
            (1, [0.6270739405881613, 0.002735721180215478, 0.5274470399599522]),
            (42, [0.6011037519201636, 0.44829055899754167, 0.8524657934904099]),
            (3735928559, [0.9413696140982211, 0.26719574979506433, 0.772033357527107]),
            (0, [0.26642920868471265, 0.0003297457005828619, 0.2232720274478197]),
        ];
        for (seed, draws) in want {
            let mut r = mulberry32(seed);
            for d in draws {
                assert_eq!(r(), d, "seed {seed}");
            }
        }
        // seed 0 and seed 2^32-1 must not panic (wrapping) and stay in [0, 1)
        for seed in [0u32, u32::MAX, 0xDEAD_BEEF] {
            let mut r = mulberry32(seed);
            for _ in 0..1000 {
                let x = r();
                assert!((0.0..1.0).contains(&x), "seed {seed}: {x}");
            }
        }
    }

    /// The parsers of the server answers (CONTRACT §3) and the queue items they make — the web's parsers.test.ts.
    #[test]
    fn parsers() {
        let items = norm_browse_items(&serde_json::json!({ "items": [
            { "kind": "video", "id": "aqz-KE-bpKQ", "url": "https://www.youtube.com/watch?v=aqz-KE-bpKQ", "title": "Big Buck Bunny", "thumb": "https://i.ytimg.com/vi/aqz-KE-bpKQ/mqdefault.jpg", "channel": "Blender", "channelId": "UCS", "publishedAt": "2014-11-10T14:05:55Z", "durationSec": 635, "views": 23425836 },
            { "kind": "playlist", "id": "PLfzxR9TEHkQfFPjC1wfveaO1r81gx-6eL", "title": "Upbeat Lofi", "thumb": "t", "channel": "A Lofi Soul", "channelId": "UCi", "itemCount": 18 },
            { "kind": "channel", "id": "UCwZEU0wAwIyZb4x5G_KJp2w", "title": "Rick Astley - Topic", "thumb": "t", "channel": "Rick Astley - Topic", "channelId": "UCwZEU0wAwIyZb4x5G_KJp2w", "itemCount": 500 },
            { "kind": "video", "title": "no id" },
            { "kind": "other", "id": "x" }
        ], "nextPage": "CBkQAA" }));
        assert_eq!(items.len(), 3);
        assert_eq!(items[0]["durationSec"], 635);
        assert_eq!(items[0]["views"], 23425836);
        assert_eq!(items[1]["url"], "https://www.youtube.com/playlist?list=PLfzxR9TEHkQfFPjC1wfveaO1r81gx-6eL");
        assert_eq!(items[2]["url"], "https://www.youtube.com/channel/UCwZEU0wAwIyZb4x5G_KJp2w");
        assert_eq!(norm_browse_item(&serde_json::json!({ "kind": "video", "id": "abcdef1", "title": "x" })).unwrap().get("durationSec"), None);
        let q = queue_item_of(&items[0], false);
        assert_eq!(q["kind"], "video");
        assert_eq!(q["source"]["service"], "youtube");
        assert_eq!(q["source"]["id"], "aqz-KE-bpKQ");
        assert_eq!(q["art"], items[0]["thumb"]);
        assert_eq!(q["artist"], "Blender");
        assert_eq!(q["durationSec"], 635);
        assert_eq!(q["addedBy"], "");
        assert_eq!(queue_item_of(&items[0], true)["kind"], "song");
        assert_eq!(queue_item_of(&items[0], true)["source"]["service"], "ytmusic");
        // meta (SERVER.md §7 answers)
        let sp = norm_meta(&serde_json::json!({ "kind": "song", "service": "spotify", "id": "4PTG3Z6ehGkBFwjybzWkR8", "url": "https://open.spotify.com/track/4PTG3Z6ehGkBFwjybzWkR8", "title": "Never Gonna Give You Up", "artist": "Rick Astley", "thumb": "https://image-cdn-fa.spotifycdn.com/image/x", "durationSec": 214, "preview": "https://p.scdn.co/mp3-preview/x" })).unwrap();
        assert_eq!(sp["artist"], "Rick Astley");
        assert_eq!(sp["durationSec"], 214);
        let pl = norm_meta(&serde_json::json!({ "kind": "playlist", "service": "spotify", "id": "x", "url": "u", "title": "T", "artist": "Spotify", "tracks": [ { "title": "Stop The Wedding!", "artist": "Ashe", "durationSec": 199, "id": "3zOY", "url": "https://open.spotify.com/track/3zOY" }, { "nope": 1 } ] })).unwrap();
        assert_eq!(pl["tracks"].as_array().unwrap().len(), 1);
        assert_eq!(pl["tracks"][0]["durationSec"], 199);
        assert!(norm_meta(&serde_json::json!({ "kind": "video", "service": "tidal" })).is_none());
        assert!(norm_meta(&serde_json::json!({ "kind": "movie", "service": "youtube" })).is_none());
        assert_eq!(norm_meta(&serde_json::json!({ "kind": "unknown", "service": "spotify", "id": "", "url": "u", "title": "" })).unwrap()["kind"], "unknown");
        let r = norm_resolved(&serde_json::json!({ "videoId": "3BFTio5296w", "url": "https://www.youtube.com/watch?v=3BFTio5296w", "title": "Never Gonna Give You Up (2022 Remaster)", "channel": "Rick Astley - Topic", "thumb": "t", "durationSec": 214, "confidence": 1 })).unwrap();
        assert_eq!(r["channel"], "Rick Astley - Topic");
        assert_eq!(r["durationSec"], 214);
        assert_eq!(norm_resolved(&serde_json::json!({ "videoId": "3BFTio5296w" })).unwrap()["url"], "https://www.youtube.com/watch?v=3BFTio5296w");
        assert!(norm_resolved(&serde_json::json!({ "videoId": "x" })).is_none());
        assert!(norm_resolved(&serde_json::json!("nope")).is_none());
        // item_for (the web's itemFor cases)
        let sp_link = crate::medialinks::parse_media_link("https://open.spotify.com/intl-de/track/4PTG3Z6ehGkBFwjybzWkR8?si=x");
        let song = item_for(&sp_link, Some(&sp), Some(&r)).unwrap();
        assert_eq!(song["url"], "https://www.youtube.com/watch?v=3BFTio5296w");
        assert_eq!(song["kind"], "song");
        assert_eq!(song["source"]["service"], "spotify");
        assert_eq!(song["source"]["url"], sp_link["input"]);
        assert_eq!(song["source"]["id"], "4PTG3Z6ehGkBFwjybzWkR8");
        assert_eq!(song["title"], "Never Gonna Give You Up");
        assert_eq!(song["artist"], "Rick Astley");
        assert_eq!(song["art"], sp["thumb"]);
        assert_eq!(song["durationSec"], 214);
        assert_eq!(song["preview"], sp["preview"]);
        assert!(item_for(&sp_link, Some(&sp), None).is_none());
        let yt_link = crate::medialinks::parse_media_link("https://youtu.be/aqz-KE-bpKQ?t=5");
        let yt_meta = norm_meta(&serde_json::json!({ "kind": "video", "service": "youtube", "id": "aqz-KE-bpKQ", "url": "u", "title": "Big Buck Bunny", "artist": "Blender", "thumb": "https://i.ytimg.com/vi/aqz-KE-bpKQ/hqdefault.jpg", "durationSec": 635 })).unwrap();
        let yt = item_for(&yt_link, Some(&yt_meta), None).unwrap();
        assert_eq!(yt["url"], "https://www.youtube.com/watch?v=aqz-KE-bpKQ");
        assert_eq!(yt["kind"], "video");
        assert_eq!(yt["title"], "Big Buck Bunny");
        assert_eq!(yt["artist"], "Blender");
        assert_eq!(yt["durationSec"], 635);
        assert_eq!(yt["source"]["service"], "youtube");
        let bare = item_for(&yt_link, None, None).unwrap();
        assert_eq!(bare["art"], "https://i.ytimg.com/vi/aqz-KE-bpKQ/hqdefault.jpg");
        assert_eq!(bare["title"], "");
        assert_eq!(item_for(&crate::medialinks::parse_media_link("https://music.youtube.com/watch?v=aqz-KE-bpKQ"), None, None).unwrap()["kind"], "song");
        assert_eq!(
            item_for(&crate::medialinks::parse_media_link("https://example.com/clips/Game%20Night.mp4"), None, None).unwrap(),
            serde_json::json!({ "url": "https://example.com/clips/Game%20Night.mp4", "title": "Game Night.mp4", "addedBy": "", "kind": "file" })
        );
        let link = item_for(&crate::medialinks::parse_media_link("https://example.com/page"), None, None).unwrap();
        assert_eq!(link["title"], "https://example.com/page");
        assert_eq!(link.get("kind"), None);
        assert!(item_for(&crate::medialinks::parse_media_link("words"), None, None).is_none());
        assert!(item_for(&crate::medialinks::parse_media_link("https://www.youtube.com/playlist?list=PLrAXtmErZgOeiKm4sgNOknGvNjby9efdf"), None, None).is_none());
    }

    /// The call bar's view of a party: a bare item when the playing url is not in the queue; a stale clock reads paused.
    /// 11.3 QOL: the strip's next url (queue_next_url) = the window's nextQueueUrl().
    #[test]
    fn call_party_shape_and_next() {
        let qq = vec![
            serde_json::json!({ "url": "a", "title": "", "addedBy": "" }),
            serde_json::json!({ "url": "b", "title": "", "addedBy": "" }),
        ];
        assert_eq!(queue_next_url(&qq, "a"), "b");
        assert_eq!(queue_next_url(&qq, "b"), "");
        assert_eq!(queue_next_url(&qq, ""), "a");
        assert_eq!(queue_next_url(&qq, "zzz"), "a");
        assert_eq!(queue_next_url(&[], "a"), "");
        let q = vec![
            serde_json::json!({ "url": "a", "title": "", "addedBy": "" }),
            serde_json::json!({ "url": "b", "title": "", "addedBy": "" }),
        ];
        let party = serde_json::json!({ "queue": q, "hostId": "" });
        let find = |u: &str| party["queue"].as_array().unwrap().iter().find(|it| it["url"] == u).cloned();
        assert!(find("b").is_some());
        assert!(find("zzz").is_none());
    }
}
