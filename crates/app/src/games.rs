//! Game Reel — "What do we play tonight" (9.0; the Game Reel DESIGN.md). The CONTROLLER behind the flyout, the
//! My-games editor and the hidden admin panel; every number is computed in games_model.rs.
//!
//! Storage = four custom room-STATE events in the CONTAINER (the room's parent space — exactly
//! the calendar's container, `calendar::container_for_room`), unencrypted on purpose (§10):
//!   • `dev.tinhat.games.prefs`    state_key = the person's user id (Matrix lets a sender write
//!       an @-key only for itself — nobody can edit your lists)                        §7.1
//!   • `dev.tinhat.games.spin`     state_key = `sp-<ms>-<hex>`, self-contained + verifiable  §7.2
//!   • `dev.tinhat.games.settings` state_key = "" — every §6 tunable + `rev`. ADMIN-ONLY: its
//!       power level is deliberately never lowered (Matrix state history = the audit log)  §7.3
//!   • `dev.tinhat.games.adjust`   state_key "admin:<target user id>" — admin credit adjustments
//!       §7.4 (10.5.2; the target's own @-key, used before, is refused by Matrix for anyone else, so
//!       admins could only ever adjust themselves — readers still honour an admin's own @-key)
//! Only prefs + spin go through `ensure_writable`, like the calendar's three event types.
//!
//! One snapshot JSON (`snapshotJson`) is what BOTH the native flyout and the web client render.
//! It is a pure function of the last store read (`RoomData`) + the presence list QML pushes
//! (`setPresent`) + the clock, so a presence change recomputes without touching the network.
//!
//! The spin (§8) is TWO writes to one state key: the record without `winner` (its event id is
//! the seed nobody controls), then the same record again with `seed` = that event id and the
//! derived `winner` (the second write REPLACES the state event, so the seed must ride along).
//! Every reader re-derives the winner from `seed`, fetches that first event and checks the
//! pool/roster match (`verified`); a mismatch is shown, never trusted.

use core::pin::Pin;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use futures_util::future::join_all;
use matrix_sdk::ruma::events::StateEventType;
use matrix_sdk::ruma::{EventId, RoomId, UserId};
use matrix_sdk::{Client, Room};
use serde_json::{json, Value};
use tokio::task::JoinHandle;

use crate::calendar;
use crate::games_model::{self as model, Adjustment, ExplainCtx, GameWeight, OddsVisibility, Pool, Prefs, Presence, Resolved, Roster, Settings, SpinRecord};

pub const PREFS_TYPE: &str = "dev.tinhat.games.prefs";
pub const SPIN_TYPE: &str = "dev.tinhat.games.spin";
pub const SETTINGS_TYPE: &str = "dev.tinhat.games.settings";
pub const ADJUST_TYPE: &str = "dev.tinhat.games.adjust";

/// §7.2: if no calendar write-back has appeared this long after a spin, any client writes it.
const WRITEBACK_FALLBACK_MS: u64 = 30_000;
/// The local-store poll that notices other people's spins/prefs (state arrives via the sync loop).
const POLL_MS: u64 = 5_000;
/// Past sessions shown in the log.
const LOG_LIMIT: usize = 40;
/// Seed events fetched per refresh beyond the current session's (older ones stay unverified
/// until scrolled into — they only feed the log).
const SEED_FETCH_LIMIT: usize = 30;
/// Catalog lookups per refresh (the rest get placeholders and are fetched next time).
const CATALOG_FETCH_LIMIT: usize = 48;
/// A failed catalog lookup is retried after this long.
const CATALOG_RETRY_MS: u64 = 5 * 60_000;
/// A failed seed fetch is retried after this long.
const SEED_RETRY_MS: u64 = 60_000;

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
        #[qproperty(QString, container_id)] //    QML: containerId — the space whose game night this is
        #[qproperty(QString, my_id)] //           QML: myId
        #[qproperty(QString, session_id)] //      QML: sessionId — the calendar session the flyout is attached to
        #[qproperty(QString, snapshot_json)] //   QML: snapshotJson — {session, roster, tiers, spin, canSpin, canVeto, respinsLeft, log, names, …}
        #[qproperty(QString, my_prefs_json)] //   QML: myPrefsJson — my §7.1 prefs ({} = none)
        #[qproperty(QString, settings_json)] //   QML: settingsJson — the flat §6 object (+rev), defaults filled in
        #[qproperty(QString, search_json)] //     QML: searchJson — catalog autocomplete results; also [entry] after addCustomGame
        #[qproperty(QString, import_json)] //     QML: importJson — Steam library import result ({error} on failure)
        #[qproperty(QString, simulation_json)] // QML: simulationJson — a snapshot under unsaved settings (admin simulator)
        #[qproperty(QString, catalog_base)] //    QML: catalogBase — `<api>/games` of the user's server (art = <base>/art/<id>/portrait)
        #[qproperty(bool, is_admin)] //           QML: isAdmin — my power level ≥ settings admin.min_pl
        #[qproperty(bool, busy)] //               QML: busy — a refresh / write is in flight
        #[qproperty(bool, resetting)] //          QML: resetting — an admin "Reset spins" is writing (10.5.2)
        #[qproperty(QString, last_error)] //      QML: lastError — friendly message ("" = ok)
        #[qproperty(bool, feature_enabled)] //    QML: featureEnabled — settings feature.enabled (kill switch)
        type GamesController = super::GamesControllerRust;

        /// QML: start() — begin the store poll that notices other people's spins (once).
        #[qinvokable]
        fn start(self: Pin<&mut GamesController>);
        /// QML: setRoom(roomId) — the open room changed: resolve its container (space) + refresh.
        #[qinvokable]
        fn set_room(self: Pin<&mut GamesController>, room_id: &QString);
        /// QML: setSession(eventId) — attach to a calendar session (the Today card's / a chat card's).
        #[qinvokable]
        fn set_session(self: Pin<&mut GamesController>, event_id: &QString);
        /// QML: setPresent(json) — `[mxid…]` of people in the call + joining soon; recomputes locally.
        #[qinvokable]
        fn set_present(self: Pin<&mut GamesController>, json: &QString);
        /// QML: refresh() — re-read the store + catalog and rebuild the snapshot.
        #[qinvokable]
        fn refresh(self: Pin<&mut GamesController>);
        /// QML: savePrefs(json) — my §7.1 prefs ({wouldPlay, favorites, definitelyNot, flexible}); {} clears.
        #[qinvokable]
        fn save_prefs(self: Pin<&mut GamesController>, json: &QString);
        /// QML: searchCatalog(q) — autocomplete → searchJson (debounced; empty q clears).
        #[qinvokable]
        fn search_catalog(self: Pin<&mut GamesController>, q: &QString);
        /// QML: addCustomGame(name) — POST /custom (bearer = my Matrix token) → searchJson = [entry].
        #[qinvokable]
        fn add_custom_game(self: Pin<&mut GamesController>, name: &QString);
        /// QML: importSteamLibrary(profile, multiplayerOnly) — POST /steam/library → importJson.
        #[qinvokable]
        fn import_steam_library(self: Pin<&mut GamesController>, profile: &QString, multiplayer_only: bool);
        /// QML: spin() — tonight's draw (§8), when canSpin.
        #[qinvokable]
        fn spin(self: Pin<&mut GamesController>);
        /// QML: veto() — use my post-spin veto on the current winner and re-spin without it (§3.6).
        #[qinvokable]
        fn veto(self: Pin<&mut GamesController>);
        /// QML: respin() — re-spin keeping the current vetoes (counts against veto.respin_limit).
        #[qinvokable]
        fn respin(self: Pin<&mut GamesController>);
        /// QML: saveSettings(json) — admin: partial §6 object merged over the current settings, rev+1.
        #[qinvokable]
        fn save_settings(self: Pin<&mut GamesController>, json: &QString);
        /// QML: adjustCredit(mxid, delta, reason) — admin: ±N on one person's derived credit (§7.4).
        #[qinvokable]
        fn adjust_credit(self: Pin<&mut GamesController>, mxid: &QString, delta: i32, reason: &QString);
        /// QML: voidSpin(spinId, reason) — admin: mark a spin voided (excluded from credit/log totals).
        #[qinvokable]
        fn void_spin(self: Pin<&mut GamesController>, spin_id: &QString, reason: &QString);
        /// QML: resetCredits() — admin: zero everyone's credit as of now.
        #[qinvokable]
        fn reset_credits(self: Pin<&mut GamesController>);
        /// QML: zeroCredit(mxid) — admin (10.5.2, Tune → Credit → Zero): one person's credit back to 0 as of now.
        #[qinvokable]
        fn zero_credit(self: Pin<&mut GamesController>, mxid: &QString);
        /// QML: setSpinCredit(spinId, counts) — admin (10.5.2): "Don't count for credit" (false) /
        /// "Count for credit" (true) on one spin; the result stands and still verifies.
        #[qinvokable]
        fn set_spin_credit(self: Pin<&mut GamesController>, spin_id: &QString, counts: bool);
        /// QML: resetSpins() — admin (10.5.2, the web's "Reset spins"): void tonight's spins and stop
        /// their re-spins counting, so the next spin starts fresh (games_model::reset_writes).
        #[qinvokable]
        fn reset_spins(self: Pin<&mut GamesController>);
        /// QML: simulate(settingsJson) — dry-run tonight under unsaved settings → simulationJson. Never writes.
        #[qinvokable]
        fn simulate(self: Pin<&mut GamesController>, settings_json: &QString);
        /// QML: clearError()
        #[qinvokable]
        fn clear_error(self: Pin<&mut GamesController>);
    }

    impl cxx_qt::Threading for GamesController {}
}

type QtThread = cxx_qt::CxxQtThread<qobject::GamesController>;

pub struct GamesControllerRust {
    container_id: QString,
    my_id: QString,
    session_id: QString,
    snapshot_json: QString,
    my_prefs_json: QString,
    settings_json: QString,
    search_json: QString,
    import_json: QString,
    simulation_json: QString,
    catalog_base: QString,
    is_admin: bool,
    busy: bool,
    resetting: bool,
    last_error: QString,
    feature_enabled: bool,
    room_id: String,
    started: bool,
    generation: u64,
    /// Who QML says is here (in the call / joining soon), as user ids.
    present: Vec<String>,
    /// The last store read; the snapshot is recomputed from it on every presence/session change.
    data: Option<Arc<RoomData>>,
    /// The most recent in-flight catalog search, so a new keystroke aborts the previous one.
    inflight_search: Mutex<Option<JoinHandle<()>>>,
    /// A spin/veto/respin write is in progress (a second click must not double-spin).
    spinning: bool,
}

impl Default for GamesControllerRust {
    fn default() -> Self {
        Self {
            container_id: QString::default(),
            my_id: QString::default(),
            session_id: QString::default(),
            snapshot_json: QString::from("{}"),
            my_prefs_json: QString::from("{}"),
            settings_json: QString::from(&Settings::default().to_json().to_string()),
            search_json: QString::from("[]"),
            import_json: QString::from("{}"),
            simulation_json: QString::from("{}"),
            catalog_base: QString::from(&default_catalog_base()),
            is_admin: false,
            busy: false,
            resetting: false,
            last_error: QString::default(),
            feature_enabled: true,
            room_id: String::new(),
            started: false,
            generation: 0,
            present: Vec::new(),
            data: None,
            inflight_search: Mutex::new(None),
            spinning: false,
        }
    }
}

/// `TINHAT_GAMES_CATALOG` overrides; else the user's server's API `/games` (§7.6; proxy.rs).
pub(crate) fn default_catalog_base() -> String {
    std::env::var("TINHAT_GAMES_CATALOG")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("{}/games", crate::proxy::base()))
        .trim_end_matches('/')
        .to_string()
}

/// What the background poll needs to know about the controller (it runs off the Qt thread).
#[derive(Default)]
struct PollState {
    cid: String,
    session_id: String,
    /// epoch ms when the §7.2 30 s write-back fallback is due (0 = nothing pending)
    writeback_due: u64,
    /// fingerprint of the container's game-night state at the last refresh
    fingerprint: u64,
}

fn poll_state() -> &'static Mutex<PollState> {
    static S: OnceLock<Mutex<PollState>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(PollState::default()))
}

impl qobject::GamesController {
    pub fn start(mut self: Pin<&mut Self>) {
        if self.rust().started {
            return;
        }
        self.as_mut().rust_mut().started = true;
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            poll_loop(qt).await;
        });
    }

    pub fn set_room(mut self: Pin<&mut Self>, room_id: &QString) {
        let rid = room_id.to_string();
        if rid == self.rust().room_id {
            return;
        }
        self.as_mut().rust_mut().room_id = rid.clone();
        if rid.is_empty() {
            return;
        }
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let Some(client) = crate::session::client() else { return };
            let Some(cid) = calendar::container_for_room(&client, &rid).await else { return };
            let _ = qt.queue(move |mut o| {
                if o.rust().room_id != rid {
                    return; // the user moved on; a later set_room owns the container now
                }
                if cid != o.container_id().to_string() {
                    o.as_mut().set_container_id(QString::from(&cid));
                    o.as_mut().rust_mut().data = None;
                    o.as_mut().set_snapshot_json(QString::from("{}"));
                    if let Ok(mut ps) = poll_state().lock() {
                        ps.cid = cid.clone();
                        ps.fingerprint = 0;
                        ps.writeback_due = 0;
                    }
                }
                o.as_mut().refresh();
            });
        });
    }

    pub fn set_session(mut self: Pin<&mut Self>, event_id: &QString) {
        let id = event_id.to_string();
        if id == self.session_id().to_string() {
            return;
        }
        self.as_mut().set_session_id(QString::from(&id));
        if let Ok(mut ps) = poll_state().lock() {
            ps.session_id = id;
        }
        // Another session's spins need their seeds verified: a full refresh, not just a recompute.
        self.refresh();
    }

    pub fn set_present(mut self: Pin<&mut Self>, json: &QString) {
        let list: Vec<String> = serde_json::from_str::<Vec<Value>>(&json.to_string())
            .unwrap_or_default()
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()).or_else(|| v.get("mxid").and_then(Value::as_str).map(|s| s.to_string())))
            .filter(|s| s.starts_with('@'))
            .collect();
        let mut dedup: Vec<String> = Vec::new();
        for p in list {
            if !dedup.contains(&p) {
                dedup.push(p);
            }
        }
        if dedup == self.rust().present {
            return;
        }
        self.as_mut().rust_mut().present = dedup;
        let c = self.as_mut().recompute();
        // 10.5.1 open roster: the pool above is already live; someone the last read never named
        // (no prefs / RSVP / spin on record) gets a re-read so the header shows who they are.
        if let Some(d) = self.rust().data.clone() {
            if self.rust().present.iter().any(|p| !d.people.contains_key(p)) {
                self.as_mut().refresh();
            }
        }
        // §6 roster.grace_min: someone who arrives within the window after MY spin joins the
        // roster for credit. Only the spinner's client appends (`late`), the pool is untouched,
        // and verification compares the roster minus `late` with the seed event.
        let (Some(c), Some(data)) = (c, self.rust().data.clone()) else { return };
        let Some(st) = c.standing else { return };
        // 10.5.1 open roster: outside a call the roster is the planned-roster PREVIEW (RSVPs + me),
        // not arrivals — leaving the call within the grace window must not append the RSVP'd
        // no-shows as "late arrivals" (they would accrue regret credit for a night they missed).
        if data.settings.roster_presence == Presence::Open && self.rust().present.is_empty() {
            return;
        }
        let grace_ms = data.settings.roster_grace_min as u64 * 60_000;
        let now = crate::appstate::now_ms();
        if st.sender != data.me || grace_ms == 0 || st.voided.is_some() || now.saturating_sub(st.ts) > grace_ms {
            return;
        }
        let newcomers: Vec<String> = c.roster_actual.iter().filter(|p| !st.roster.contains(p)).cloned().collect();
        if newcomers.is_empty() {
            return;
        }
        let cid = self.container_id().to_string();
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            match do_late_arrivals(&cid, &st.id, &newcomers).await {
                Ok(()) => {
                    let _ = qt.queue(|mut o| o.as_mut().refresh());
                }
                Err(e) => eprintln!("[games] late arrivals not recorded: {e}"),
            }
        });
    }

    pub fn refresh(mut self: Pin<&mut Self>) {
        let cid = self.container_id().to_string();
        if cid.is_empty() {
            return;
        }
        let gen = {
            let mut r = self.as_mut().rust_mut();
            r.generation += 1;
            r.generation
        };
        let session = self.session_id().to_string();
        let present = self.rust().present.clone();
        let base = self.as_mut().fresh_catalog_base();
        self.as_mut().set_busy(true);
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let Some(client) = crate::session::client() else {
                let _ = qt.queue(|mut o| o.as_mut().set_busy(false));
                return;
            };
            let Some(data) = load_room_data(&client, &cid, &base, &session, &present).await else {
                let _ = qt.queue(|mut o| o.as_mut().set_busy(false));
                return;
            };
            // §7.2 / §11 "spinner disconnects before writing winner": 30 s after a spin with no
            // calendar write-back, any client writes it (idempotent — same content every time).
            let now = crate::appstate::now_ms();
            let c = compute(&data, &base, &session, &present, &data.settings, now);
            if let (Some((gid, name)), true) = (&c.writeback, c.writeback_due > 0 && now >= c.writeback_due) {
                if let Some(room) = container_room(&cid) {
                    write_back_calendar(&room, &c.session_id, gid, name).await;
                }
            }
            if let Ok(mut ps) = poll_state().lock() {
                ps.fingerprint = data.fingerprint;
            }
            let data = Arc::new(data);
            let _ = qt.queue(move |mut o| {
                if o.rust().generation != gen {
                    return; // superseded by a newer refresh / container switch
                }
                o.as_mut().rust_mut().data = Some(data);
                o.as_mut().recompute();
                o.as_mut().set_busy(false);
            });
        });
    }

    /// `<api>/games` as it is now. The base comes from the user's server (its announced API, else the homeserver
    /// origin), which isn't known yet when this controller is built at startup: before a fresh sign-in it was
    /// "/games" and stayed that way until a restart, so Steam import, search and art failed on the PC with nothing
    /// sent (12.0.0, a Windows sign-in after the installer's Repair). Re-read on every use; QML's art URLs follow.
    fn fresh_catalog_base(mut self: Pin<&mut Self>) -> String {
        let now = default_catalog_base();
        if now != self.catalog_base().to_string() {
            self.as_mut().set_catalog_base(QString::from(&now));
        }
        now
    }

    /// Rebuild every derived property from the cached store read + presence + the clock.
    /// Returns the computed state (for callers that need `standing` / the roster).
    fn recompute(mut self: Pin<&mut Self>) -> Option<Computed> {
        let data = self.rust().data.clone()?;
        let session = self.session_id().to_string();
        let present = self.rust().present.clone();
        let base = self.as_mut().fresh_catalog_base();
        let now = crate::appstate::now_ms();
        let c = compute(&data, &base, &session, &present, &data.settings, now);
        self.as_mut().set_my_id(QString::from(&data.me));
        self.as_mut().set_snapshot_json(QString::from(&c.json.to_string()));
        self.as_mut().set_my_prefs_json(QString::from(&c.my_prefs.to_string()));
        // 10.1 (TH-REEL-05): the admin panel reads { values, defaults } — with a flat object its
        // "(default)" hints were blank and "Reset all to defaults" blanked every field.
        let settings_payload = json!({
            "values": data.settings.to_json(),
            "defaults": crate::games_model::Settings::default().to_json(),
        });
        self.as_mut().set_settings_json(QString::from(&settings_payload.to_string()));
        self.as_mut().set_is_admin(c.is_admin);
        self.as_mut().set_feature_enabled(data.settings.feature_enabled);
        if let Ok(mut ps) = poll_state().lock() {
            ps.writeback_due = c.writeback_due;
        }
        Some(c)
    }

    pub fn save_prefs(mut self: Pin<&mut Self>, json: &QString) {
        let cid = self.container_id().to_string();
        let raw = json.to_string();
        if cid.is_empty() {
            return;
        }
        self.as_mut().set_last_error(QString::default());
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            match do_save_prefs(&cid, &raw).await {
                Ok(()) => {
                    let _ = qt.queue(|mut o| o.as_mut().refresh());
                }
                Err(msg) => {
                    eprintln!("[games] prefs save failed: {msg}");
                    let _ = qt.queue(move |mut o| o.as_mut().set_last_error(QString::from(&msg)));
                }
            }
        });
    }

    pub fn search_catalog(mut self: Pin<&mut Self>, q: &QString) {
        let q = q.to_string().trim().to_string();
        let base = self.as_mut().fresh_catalog_base();
        // selfhost: a server without the catalogue (games: false) answers no search; games are added by name
        let q = if crate::server_offer::feature_on_global(crate::server_offer::ServerFeature::Games) { q } else { String::new() };
        if q.is_empty() {
            self.as_mut().set_search_json(QString::from("[]"));
            if let Ok(mut g) = self.rust().inflight_search.lock() {
                if let Some(prev) = g.take() {
                    prev.abort();
                }
            }
            return;
        }
        let qt = self.qt_thread();
        let handle = crate::bridge::runtime().spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await; // typing debounce
            let token = crate::session::client().and_then(|c| c.access_token());
            let res = catalog_get_auth(&base, &format!("/search?q={}", enc(&q)), token.as_deref()).await;
            let _ = qt.queue(move |mut o| match res {
                Ok(v) => {
                    // The service answers { "items": [...] } (TH-REEL-01); "results" kept as a fallback.
                    let list = v.get("items").or_else(|| v.get("results")).cloned().unwrap_or(v);
                    let list = if list.is_array() { list } else { json!([]) };
                    o.as_mut().set_search_json(QString::from(&list.to_string()));
                }
                Err(e) => {
                    eprintln!("[games] search failed: {e}");
                    o.as_mut().set_search_json(QString::from("[]"));
                    o.as_mut().set_last_error(QString::from("Couldn't reach the game catalog"));
                }
            });
        });
        if let Ok(mut g) = self.rust().inflight_search.lock() {
            if let Some(prev) = g.replace(handle) {
                prev.abort();
            }
        }
    }

    pub fn add_custom_game(mut self: Pin<&mut Self>, name: &QString) {
        let name = name.to_string().trim().to_string();
        let base = self.as_mut().fresh_catalog_base();
        if name.is_empty() {
            return;
        }
        self.as_mut().set_last_error(QString::default());
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            // The catalog service checks the Matrix token so only signed-in members add entries.
            let token = crate::session::client().and_then(|c| c.access_token());
            let res = catalog_post(&base, "/custom", json!({ "name": name }), token.as_deref()).await;
            let _ = qt.queue(move |mut o| match res {
                Ok(v) => {
                    let entry = v.get("game").cloned().unwrap_or(v);
                    if let Some(id) = entry.get("id").and_then(Value::as_str) {
                        remember_catalog_entry(id, &entry);
                    }
                    o.as_mut().set_search_json(QString::from(&json!([entry]).to_string()));
                }
                Err(e) => {
                    eprintln!("[games] custom game failed: {e}");
                    o.as_mut().set_last_error(QString::from(&format!("Couldn't add that game: {e}")));
                }
            });
        });
    }

    pub fn import_steam_library(mut self: Pin<&mut Self>, profile: &QString, multiplayer_only: bool) {
        let profile = profile.to_string().trim().to_string();
        let base = self.as_mut().fresh_catalog_base();
        if profile.is_empty() {
            return;
        }
        if !crate::server_offer::feature_on_global(crate::server_offer::ServerFeature::Steam) {
            self.as_mut().set_last_error(QString::from("This server doesn't import Steam libraries."));
            return;
        }
        self.as_mut().set_last_error(QString::default());
        self.as_mut().set_import_json(QString::from("{\"loading\":true}"));
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            // §10: the service resolves the profile, returns the list and forgets it. Nothing
            // about the person is stored anywhere — not even here.
            // Authenticated like /custom: the service only answers signed-in members. Without the
            // token every import came back 401 and the whole first-run setup flow was unusable.
            let token = crate::session::client().and_then(|c| c.access_token());
            let res = catalog_post(&base, "/steam/library", json!({ "profile": profile, "multiplayerOnly": multiplayer_only }), token.as_deref()).await;
            let _ = qt.queue(move |mut o| match res {
                Ok(v) => {
                    // The service returns { items, total, pending } — not { games }.
                    if let Some(games) = v.get("items").and_then(Value::as_array) {
                        for g in games {
                            if let Some(id) = g.get("id").and_then(Value::as_str) {
                                remember_catalog_entry(id, g);
                            }
                        }
                    }
                    o.as_mut().set_import_json(QString::from(&v.to_string()));
                }
                Err(e) => {
                    eprintln!("[games] steam import failed: {e}");
                    o.as_mut().set_import_json(QString::from(&json!({ "error": e }).to_string()));
                    o.as_mut().set_last_error(QString::from("Steam import didn't work — see the note in the editor"));
                }
            });
        });
    }

    pub fn spin(self: Pin<&mut Self>) {
        self.run_spin(SpinMode::Initial);
    }

    pub fn veto(self: Pin<&mut Self>) {
        self.run_spin(SpinMode::Veto);
    }

    pub fn respin(self: Pin<&mut Self>) {
        self.run_spin(SpinMode::Respin);
    }

    fn run_spin(mut self: Pin<&mut Self>, mode: SpinMode) {
        let cid = self.container_id().to_string();
        let session = self.session_id().to_string();
        let present = self.rust().present.clone();
        let base = self.as_mut().fresh_catalog_base();
        // 10.5.1: an empty session is fine — do_spin resolves it to tonight's ad-hoc session.
        // 10.5.2: not while an admin reset is rewriting tonight (the web disables Spin too).
        if cid.is_empty() || self.rust().spinning || *self.resetting() {
            return;
        }
        self.as_mut().rust_mut().spinning = true;
        self.as_mut().set_last_error(QString::default());
        self.as_mut().set_busy(true);
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let res = do_spin(&cid, &base, &session, &present, mode).await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().rust_mut().spinning = false;
                if let Err(msg) = res {
                    eprintln!("[games] spin failed: {msg}");
                    o.as_mut().set_last_error(QString::from(&msg));
                }
                o.as_mut().refresh();
            });
        });
    }

    pub fn save_settings(mut self: Pin<&mut Self>, json: &QString) {
        let cid = self.container_id().to_string();
        let raw = json.to_string();
        if cid.is_empty() {
            return;
        }
        if !self.is_admin() {
            self.as_mut().set_last_error(QString::from("Only a space admin can tune game night"));
            return;
        }
        self.as_mut().set_last_error(QString::default());
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            match do_save_settings(&cid, &raw).await {
                Ok(()) => {
                    let _ = qt.queue(|mut o| o.as_mut().refresh());
                }
                Err(msg) => {
                    let _ = qt.queue(move |mut o| o.as_mut().set_last_error(QString::from(&msg)));
                }
            }
        });
    }

    pub fn adjust_credit(mut self: Pin<&mut Self>, mxid: &QString, delta: i32, reason: &QString) {
        let cid = self.container_id().to_string();
        let mxid = mxid.to_string();
        let reason = reason.to_string();
        if cid.is_empty() || !mxid.starts_with('@') || delta == 0 {
            return;
        }
        if !self.is_admin() {
            self.as_mut().set_last_error(QString::from("Only a space admin can adjust credit"));
            return;
        }
        self.as_mut().set_last_error(QString::default());
        // 10.5.2: busy from the write until the re-read — the credit buttons stay disabled, so two
        // clicks can't both read the same cumulative adjustment and overwrite each other
        self.as_mut().set_busy(true);
        let session = self.session_id().to_string();
        let present = self.rust().present.clone();
        let base = self.as_mut().fresh_catalog_base();
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let res = do_adjust(&cid, &base, &session, &present, &mxid, delta as i64, &reason).await;
            let _ = qt.queue(move |mut o| {
                if let Err(msg) = res {
                    o.as_mut().set_last_error(QString::from(&msg));
                }
                o.as_mut().refresh();
            });
        });
    }

    pub fn void_spin(mut self: Pin<&mut Self>, spin_id: &QString, reason: &QString) {
        let cid = self.container_id().to_string();
        let spin_id = spin_id.to_string();
        let reason = reason.to_string();
        if cid.is_empty() || spin_id.is_empty() {
            return;
        }
        if !self.is_admin() {
            self.as_mut().set_last_error(QString::from("Only a space admin can void a spin"));
            return;
        }
        self.as_mut().set_last_error(QString::default());
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            match do_void(&cid, &spin_id, &reason).await {
                Ok(()) => {
                    let _ = qt.queue(|mut o| o.as_mut().refresh());
                }
                Err(msg) => {
                    let _ = qt.queue(move |mut o| o.as_mut().set_last_error(QString::from(&msg)));
                }
            }
        });
    }

    pub fn reset_credits(mut self: Pin<&mut Self>) {
        let cid = self.container_id().to_string();
        if cid.is_empty() {
            return;
        }
        if !self.is_admin() {
            self.as_mut().set_last_error(QString::from("Only a space admin can reset credits"));
            return;
        }
        self.as_mut().set_last_error(QString::default());
        self.as_mut().set_busy(true);
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let res = do_reset_credits(&cid).await;
            let _ = qt.queue(move |mut o| {
                if let Err(msg) = res {
                    o.as_mut().set_last_error(QString::from(&msg));
                }
                o.as_mut().refresh();
            });
        });
    }

    /// 10.5.2 (Tune → Credit → Zero): one person's credit back to 0 as of now, at "admin:<mxid>".
    pub fn zero_credit(mut self: Pin<&mut Self>, mxid: &QString) {
        let cid = self.container_id().to_string();
        let mxid = mxid.to_string();
        if cid.is_empty() || !mxid.starts_with('@') {
            return;
        }
        if !self.is_admin() {
            self.as_mut().set_last_error(QString::from("Only a space admin can reset credits"));
            return;
        }
        self.as_mut().set_last_error(QString::default());
        self.as_mut().set_busy(true);
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let res = do_zero_credit(&cid, &mxid).await;
            let _ = qt.queue(move |mut o| {
                if let Err(msg) = res {
                    o.as_mut().set_last_error(QString::from(&msg));
                }
                o.as_mut().refresh();
            });
        });
    }

    /// 10.5.2 admin "Don't count for credit" / "Count for credit" on one spin (tonight's result or a
    /// past night's): the record keeps its result and still verifies; only credit ignores it.
    pub fn set_spin_credit(mut self: Pin<&mut Self>, spin_id: &QString, counts: bool) {
        let cid = self.container_id().to_string();
        let spin_id = spin_id.to_string();
        if cid.is_empty() || spin_id.is_empty() {
            return;
        }
        if !self.is_admin() {
            self.as_mut().set_last_error(QString::from("Only a space admin can change what counts for credit"));
            return;
        }
        self.as_mut().set_last_error(QString::default());
        self.as_mut().set_busy(true);
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let res = do_set_spin_credit(&cid, &spin_id, counts).await;
            let _ = qt.queue(move |mut o| {
                if let Err(msg) = res {
                    o.as_mut().set_last_error(QString::from(&msg));
                }
                o.as_mut().refresh();
            });
        });
    }

    /// 10.5.2 admin "Reset spins" (the web's gamesStore `resetSpins`, 2026-09-23 — an admin hit the
    /// re-spin limit while testing and had no way back): tonight's spins are voided and the re-spins
    /// used stop counting, so the next spin starts fresh — full re-spins, vetoes back. Tune's
    /// "Void tonight's spin" is not that: it voids only the standing record, which falls back to the
    /// previous result with the count unchanged. Wire form + order: `games_model::reset_writes`.
    pub fn reset_spins(mut self: Pin<&mut Self>) {
        let cid = self.container_id().to_string();
        if cid.is_empty() || self.rust().spinning || *self.resetting() {
            return;
        }
        if !self.is_admin() {
            self.as_mut().set_last_error(QString::from("Only a space admin can reset tonight's spins"));
            return;
        }
        let session = self.session_id().to_string();
        let present = self.rust().present.clone();
        let base = self.as_mut().fresh_catalog_base();
        self.as_mut().set_last_error(QString::default());
        self.as_mut().set_resetting(true);
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let res = do_reset_spins(&cid, &base, &session, &present).await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_resetting(false);
                if let Err(msg) = res {
                    eprintln!("[games] reset spins failed: {msg}");
                    o.as_mut().set_last_error(QString::from(&msg));
                }
                o.as_mut().refresh();
            });
        });
    }

    /// The admin simulator (§4.4): the same snapshot, under `settings_json` laid over the saved
    /// settings, for the current roster. Pure — nothing is written or fetched.
    pub fn simulate(mut self: Pin<&mut Self>, settings_json: &QString) {
        let Some(data) = self.rust().data.clone() else {
            self.as_mut().set_simulation_json(QString::from("{}"));
            return;
        };
        let patch: Value = serde_json::from_str(&settings_json.to_string()).unwrap_or(json!({}));
        let settings = data.settings.merged(&patch);
        let session = self.session_id().to_string();
        let present = self.rust().present.clone();
        let base = self.as_mut().fresh_catalog_base();
        let c = compute(&data, &base, &session, &present, &settings, crate::appstate::now_ms());
        self.as_mut().set_simulation_json(QString::from(&c.json.to_string()));
    }

    pub fn clear_error(mut self: Pin<&mut Self>) {
        self.as_mut().set_last_error(QString::default());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SpinMode {
    Initial,
    Veto,
    Respin,
}

// ---------------------------------------------------------------------------------------------
// Data layer: one read of everything the snapshot needs
// ---------------------------------------------------------------------------------------------

struct Person {
    name: String,
    avatar: String,
}

#[derive(Clone, Debug, Default)]
struct CatalogEntry {
    name: String,
    year: u64,
    /// epoch ms of the last lookup attempt (0 = never)
    fetched_at: u64,
    /// the lookup answered (false = placeholder, retried after CATALOG_RETRY_MS)
    ok: bool,
}

pub(crate) struct RoomData {
    me: String,
    my_pl: i64,
    /// I may send spin records here (after PL provisioning)
    can_write: bool,
    settings: Settings,
    prefs: HashMap<String, Prefs>,
    adjustments: HashMap<String, Adjustment>,
    /// every spin record in the container, seed ts + verification resolved, oldest first
    spins: Vec<SpinRecord>,
    /// calendar events (id → content)
    events: HashMap<String, Value>,
    /// calendar RSVPs (event id → [(mxid, status)])
    rsvps: HashMap<String, Vec<(String, String)>>,
    people: HashMap<String, Person>,
    catalog: HashMap<String, CatalogEntry>,
    /// the calendar's curated games (slug → {name, color, server, catalogId}) — join info (§4.2)
    cal_games: Vec<Value>,
    /// hash of the state event ids that feed this read (the poll's change detector)
    fingerprint: u64,
}

fn container_room(cid: &str) -> Option<Room> {
    let client = crate::session::client()?;
    let rid = RoomId::parse(cid).ok()?;
    client.get_room(&rid)
}

/// Turn an SDK error into something a person can act on.
fn friendly(e: &str) -> String {
    if e.contains("M_FORBIDDEN") || e.contains("403") {
        "You can't write game-night state here yet — ask a space admin to open the flyout once (that unlocks it for everyone)".to_string()
    } else if e.contains("M_LIMIT_EXCEEDED") || e.contains("429") {
        "Slow down a little — the server is rate-limiting".to_string()
    } else {
        format!("Couldn't save: {e}")
    }
}

/// Every state event of a type as its full JSON (`event_id`, `origin_server_ts`, `sender`,
/// `state_key`, `content`) — spin records need the id + server clock, which
/// `appstate::get_state_all` drops.
/// The room's power levels (10.5.2: part of both fingerprints — admin-only writes count while their sender is one).
const POWER_LEVELS_TYPE: &str = "m.room.power_levels";

async fn get_state_full(room: &Room, event_type: &str) -> Vec<Value> {
    let Ok(raws) = room.get_state_events(StateEventType::from(event_type)).await else {
        return Vec::new();
    };
    raws.iter().filter_map(crate::appstate::raw_to_value).collect()
}

fn fnv(h: &mut u64, s: &str) {
    for b in s.bytes() {
        *h ^= b as u64;
        *h = h.wrapping_mul(0x100000001b3);
    }
}

/// Member name + avatar (cache-first; a missing avatar downloads once in the background).
async fn resolve_person(client: &Client, room: &Room, mxid: &str, cache: &mut HashMap<String, Person>) {
    if cache.contains_key(mxid) {
        return;
    }
    let mut name = mxid.trim_start_matches('@').split(':').next().unwrap_or(mxid).to_string();
    let mut avatar = String::new();
    if let Ok(parsed) = UserId::parse(mxid) {
        if let Ok(Some(member)) = room.get_member_no_sync(&parsed).await {
            if let Some(dn) = member.display_name() {
                name = dn.to_string();
            }
            if let Some(mxc) = member.avatar_url() {
                avatar = crate::bridge::resolve_avatar(client, mxc).await.unwrap_or_default();
            }
        }
    }
    cache.insert(mxid.to_string(), Person { name, avatar });
}

struct SeedFetch {
    at: u64,
    event: Option<Value>,
}

fn seed_cache() -> &'static Mutex<HashMap<String, SeedFetch>> {
    static C: OnceLock<Mutex<HashMap<String, SeedFetch>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The seed event (a spin's FIRST write) by id, cached for the process lifetime — it is
/// immutable history, and every refresh would otherwise re-fetch it.
async fn fetch_seed(room: &Room, event_id: &str, now: u64) -> Option<Value> {
    if let Ok(c) = seed_cache().lock() {
        if let Some(f) = c.get(event_id) {
            if f.event.is_some() || now.saturating_sub(f.at) < SEED_RETRY_MS {
                return f.event.clone();
            }
        }
    }
    let ev = match EventId::parse(event_id) {
        Ok(eid) => match room.event(&eid, None).await {
            Ok(te) => te.raw().deserialize_as::<Value>().ok(),
            Err(e) => {
                eprintln!("[games] seed {event_id} fetch failed: {e}");
                None
            }
        },
        Err(_) => None,
    };
    if let Ok(mut c) = seed_cache().lock() {
        c.insert(event_id.to_string(), SeedFetch { at: now, event: ev.clone() });
    }
    ev
}

/// Fill the derived fields of a record: the winner THIS client derives, the seed's server
/// clock, the spinner's identity (the seed's sender — a later void rewrite is the admin's),
/// and whether the record still matches its seed event (§7.2 "a mismatch is shown, not trusted").
/// `writer_is_admin` = the sender of the record's CURRENT copy is an admin now (10.5.2: only then
/// may that copy's credit flag differ from the seed's — `model::trusted_credit_flag`).
fn verify(rec: &mut SpinRecord, seed_event: Option<&Value>, writer_is_admin: bool) {
    rec.tampered = false;
    rec.computed_winner = model::derive_winner(rec.seed_id(), &rec.pool);
    let winner_ok = rec.winner.is_none() || rec.winner == rec.computed_winner;
    if rec.seed.is_empty() {
        // The record IS the first write (the spinner never got to write `winner`).
        rec.verified = winner_ok;
        return;
    }
    match seed_event {
        Some(ev) => {
            if let Some(ts) = ev.get("origin_server_ts").and_then(Value::as_u64) {
                rec.ts = ts;
            }
            if let Some(s) = ev.get("sender").and_then(Value::as_str) {
                rec.sender = s.to_string();
            }
            let first = SpinRecord::from_json(&rec.id, &rec.seed, rec.ts, &rec.sender, ev.get("content").unwrap_or(&Value::Null));
            rec.credit = model::trusted_credit_flag(rec.credit, Some(first.credit), writer_is_admin);
            // The seed event is the immutable copy, so IT decides the winner. Deriving from
            // rec.pool let a modified client reorder the pool in its second write until the
            // cumulative walk landed on its own favourite, with the seed id left untouched.
            rec.computed_winner = model::derive_winner(rec.seed_id(), &first.pool);
            let winner_ok = rec.winner.is_none() || rec.winner == rec.computed_winner;
            let same_pool = first.pool.len() == rec.pool.len()
                && first.pool.iter().zip(rec.pool.iter()).all(|(a, b)| a.0 == b.0 && (a.1 - b.1).abs() < 1e-6);
            let same_roster = first.roster == rec.roster_at_spin();
            let same_session = first.session_id == rec.session_id;
            rec.verified = same_pool && same_roster && same_session && winner_ok;
            // We looked and it disagreed: this record is not to be trusted with anything.
            rec.tampered = !rec.verified;
        }
        // Seed not fetched (only the newest are). Unknown, not forged — keep the old behaviour.
        None => rec.verified = false,
    }
}

async fn load_room_data(client: &Client, cid: &str, base: &str, session_id: &str, present: &[String]) -> Option<RoomData> {
    load_room_data_opts(client, cid, base, session_id, present, true).await
}

/// widgets W2 (the Game night widget, WIDGETS.md §23): MY banked Game Reel credit in space `cid` for session
/// `session_id` — the Reel's own computation (the room data read WITHOUT `ensure_writable`: a reader never changes the
/// room's power levels; compute's credit rows) — and the credits mode ("free" = spins don't use credit). The credit is
/// None when there is no room data; 0 when I am not listed.
pub async fn my_credit(cid: &str, session_id: &str) -> Option<(f64, String)> {
    let client = crate::session::client()?;
    let base = default_catalog_base();
    let data = load_room_data_opts(&client, cid, &base, session_id, &[], false).await?;
    let now = crate::appstate::now_ms();
    let c = compute(&data, &base, session_id, &[], &data.settings, now);
    let credit = c.json["credits"].as_array().and_then(|rows| rows.iter().find(|r| r["mxid"].as_str() == Some(data.me.as_str()))).and_then(|r| r["credit"].as_f64()).unwrap_or(0.0);
    Some((credit, c.json["creditsMode"].as_str().unwrap_or("").to_string()))
}

async fn load_room_data_opts(client: &Client, cid: &str, base: &str, session_id: &str, present: &[String], ensure: bool) -> Option<RoomData> {
    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let rid = RoomId::parse(cid).ok()?;
    let room = client.get_room(&rid)?;
    let now = crate::appstate::now_ms();
    // 10.5.1: with no (or a vanished) calendar session the reel works on tonight's ad-hoc session —
    // its records are "this session's" for seed verification too.
    let adhoc = model::adhoc_session_id(now);
    let current = |sid: &str| sid == session_id || sid == adhoc;

    // Admin-only, idempotent: unlock prefs + spin for PL-0 members (a member returns early
    // inside, no 403 noise). Settings/adjust stay at the room's admin level on purpose.
    if ensure {
        crate::appstate::ensure_writable(&room, &[PREFS_TYPE, SPIN_TYPE]).await;
    }
    let mut my_pl: i64 = 0;
    let mut can_write = false;
    // 10.5.2 (Astra release review #4): the power-level revision goes into the fingerprint (hashed after the
    // ad-hoc session id, like `fingerprint`). Read BEFORE the snapshot below: a change landing in between
    // leaves an older id here, so the next poll sees a difference and refreshes.
    let pl_ids: Vec<String> = get_state_full(&room, POWER_LEVELS_TYPE)
        .await
        .iter()
        .map(|ev| ev["event_id"].as_str().unwrap_or("").to_string())
        .collect();
    let power_levels = room.power_levels().await.ok();
    // anyone's power level (10.5.2: whether a sender of an adjustment / a spin rewrite is an admin now)
    let pl_of = |mxid: &str| -> i64 {
        use matrix_sdk::ruma::events::room::power_levels::UserPowerLevel;
        match (&power_levels, UserId::parse(mxid)) {
            (Some(pl), Ok(uid)) => match pl.for_user(&uid) {
                UserPowerLevel::Infinite => i64::MAX,
                UserPowerLevel::Int(i) => i64::from(i),
                _ => 0,
            },
            _ => 0,
        }
    };
    if let (Some(pl), Ok(uid)) = (&power_levels, UserId::parse(&me)) {
        can_write = pl.user_can_send_state(&uid, StateEventType::from(SPIN_TYPE));
        my_pl = pl_of(&me);
    }
    let mut fp: u64 = 0xcbf29ce484222325;

    let settings_raw = crate::appstate::get_state(&room, SETTINGS_TYPE, "").await.filter(Value::is_object);
    let settings = settings_raw.as_ref().map(Settings::from_json).unwrap_or_default();
    fnv(&mut fp, &settings.rev.to_string());

    let mut prefs: HashMap<String, Prefs> = HashMap::new();
    for ev in get_state_full(&room, PREFS_TYPE).await {
        let sk = ev["state_key"].as_str().unwrap_or("");
        let sender = ev["sender"].as_str().unwrap_or("");
        // Matrix already refuses an @-key not equal to the sender; belt and braces.
        if sk.is_empty() || sk != sender {
            continue;
        }
        let Some(c) = ev.get("content").filter(|c| c.as_object().map(|o| !o.is_empty()).unwrap_or(false)) else { continue };
        fnv(&mut fp, ev["event_id"].as_str().unwrap_or(""));
        prefs.insert(sk.to_string(), Prefs::from_json(c).normalized(settings.favorite_slots));
    }

    // 10.5.2: an adjustment counts only while its sender is an admin (power level ≥ admin.min_pl) —
    // the admin key "admin:<mxid>" or the legacy self-key; per person the one written last wins.
    let is_admin_now = |mxid: &str| pl_of(mxid) >= settings.admin_min_pl;
    let mut adjust_events: Vec<model::AdjustEvent> = Vec::new();
    for ev in get_state_full(&room, ADJUST_TYPE).await {
        let Some(c) = ev.get("content").filter(|c| c.as_object().map(|o| !o.is_empty()).unwrap_or(false)) else { continue };
        fnv(&mut fp, ev["event_id"].as_str().unwrap_or(""));
        adjust_events.push(model::AdjustEvent {
            state_key: ev["state_key"].as_str().unwrap_or("").to_string(),
            sender: ev["sender"].as_str().unwrap_or("").to_string(),
            ts: ev["origin_server_ts"].as_u64().unwrap_or(0),
            content: c.clone(),
        });
    }
    let adjustments: HashMap<String, Adjustment> = model::merge_adjustments(&adjust_events, &is_admin_now);
    // 10.5.2: an admin's "Don't count for credit" / "Count for credit" (admin-only state, applied below)
    let overrides = model::credit_overrides(&adjust_events, &is_admin_now);

    let mut spins: Vec<SpinRecord> = Vec::new();
    for ev in get_state_full(&room, SPIN_TYPE).await {
        let sk = ev["state_key"].as_str().unwrap_or("");
        let Some(c) = ev.get("content").filter(|c| c.as_object().map(|o| !o.is_empty()).unwrap_or(false)) else { continue };
        let eid = ev["event_id"].as_str().unwrap_or("");
        fnv(&mut fp, eid);
        let ts = ev["origin_server_ts"].as_u64().unwrap_or(0);
        let rec = SpinRecord::from_json(sk, eid, ts, ev["sender"].as_str().unwrap_or(""), c);
        if !rec.session_id.is_empty() && !rec.pool.is_empty() {
            spins.push(rec);
        }
    }
    spins.sort_by(|a, b| a.ts.cmp(&b.ts).then_with(|| a.id.cmp(&b.id)));
    // Verify: the current session's records always, the newest SEED_FETCH_LIMIT others too.
    let mut order: Vec<usize> = (0..spins.len()).collect();
    order.sort_by(|a, b| {
        let pa = current(&spins[*a].session_id);
        let pb = current(&spins[*b].session_id);
        pb.cmp(&pa).then_with(|| spins[*b].ts.cmp(&spins[*a].ts))
    });
    let mut fetched = 0usize;
    for i in order {
        let seed = spins[i].seed.clone();
        let ev = if seed.is_empty() {
            None
        } else if current(&spins[i].session_id) || fetched < SEED_FETCH_LIMIT {
            fetched += 1;
            fetch_seed(&room, &seed, now).await
        } else {
            None
        };
        // spins[i].sender is still the CURRENT copy's sender here (verify swaps in the seed's spinner)
        let writer_admin = is_admin_now(&spins[i].sender);
        verify(&mut spins[i], ev.as_ref(), writer_admin);
    }
    for s in spins.iter_mut() {
        if let Some(counts) = overrides.get(&s.id) {
            s.credit = *counts;
        }
    }
    spins.sort_by(|a, b| a.ts.cmp(&b.ts).then_with(|| a.id.cmp(&b.id)));

    let mut events: HashMap<String, Value> = HashMap::new();
    for ev in get_state_full(&room, calendar::EVENT_TYPE).await {
        let sk = ev["state_key"].as_str().unwrap_or("");
        let Some(c) = ev.get("content") else { continue };
        if c.get("title").and_then(Value::as_str).map(|t| !t.trim().is_empty()).unwrap_or(false) {
            if sk == session_id {
                fnv(&mut fp, ev["event_id"].as_str().unwrap_or(""));
            }
            events.insert(sk.to_string(), c.clone());
        }
    }
    let mut rsvps: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for ev in get_state_full(&room, calendar::RSVP_TYPE).await {
        let user = ev["state_key"].as_str().unwrap_or("");
        let Some(map) = ev.get("content").and_then(Value::as_object) else { continue };
        if map.contains_key(session_id) {
            fnv(&mut fp, ev["event_id"].as_str().unwrap_or(""));
        }
        for (eid, entry) in map {
            let status = entry.get("status").and_then(Value::as_str).unwrap_or("").to_string();
            if status.is_empty() {
                continue;
            }
            rsvps.entry(eid.clone()).or_default().push((user.to_string(), status));
        }
    }
    // 10.5.1: tonight's ad-hoc session rolls over at the fixed day boundary (14:00 UTC) — the poll
    // sees the new id (always hashed: a picked event that vanished falls back to it too).
    fnv(&mut fp, &adhoc);
    // 10.5.2 (Astra release review #4): adjustments, credit overrides and spin rewrites count only while
    // their sender is an admin NOW (pl_of above), so a power-level change must refresh the panel. Same
    // input, same place, as `fingerprint` (read before the snapshot, see pl_ids).
    for id in &pl_ids {
        fnv(&mut fp, id);
    }
    let cal_games: Vec<Value> = crate::appstate::get_state_all(&room, calendar::GAME_TYPE)
        .await
        .into_iter()
        .filter(|(_, _, c)| c.get("name").and_then(Value::as_str).map(|n| !n.is_empty()).unwrap_or(false))
        .map(|(slug, _, mut c)| {
            c["slug"] = json!(slug);
            c
        })
        .collect();

    // People: everyone the snapshot may name.
    let mut mxids: HashSet<String> = prefs.keys().cloned().collect();
    mxids.extend(adjustments.keys().cloned());
    for s in &spins {
        mxids.extend(s.roster.iter().cloned());
        mxids.insert(s.sender.clone());
        if let Some(v) = &s.voided {
            mxids.insert(v.by.clone());
        }
    }
    if let Some(list) = rsvps.get(session_id) {
        mxids.extend(list.iter().map(|(u, _)| u.clone()));
    }
    // 10.5.1 open roster: everyone in the call is on it, RSVP or not — name them too.
    mxids.extend(present.iter().cloned());
    if !me.is_empty() {
        mxids.insert(me.clone());
    }
    let mut people: HashMap<String, Person> = HashMap::new();
    for m in &mxids {
        if m.starts_with('@') {
            resolve_person(client, &room, m, &mut people).await;
        }
    }

    // Catalog: every game id anyone references (names + art live there, never in the events).
    let mut ids: Vec<String> = Vec::new();
    let mut push = |id: &str| {
        if !id.is_empty() && !ids.iter().any(|x| x == id) {
            ids.push(id.to_string());
        }
    };
    for p in prefs.values() {
        for g in p.would_play.iter().chain(p.favorites.iter()).chain(p.definitely_not.iter()) {
            push(g);
        }
    }
    for s in spins.iter().rev() {
        for (g, _) in &s.pool {
            push(g);
        }
        if let Some(w) = s.effective_winner() {
            push(w);
        }
    }
    if let Some(g) = events.get(session_id).and_then(|e| e.get("gameId")).and_then(Value::as_str) {
        push(g);
    }
    let catalog = resolve_catalog(base, &ids, now).await;

    Some(RoomData { me, my_pl, can_write, settings, prefs, adjustments, spins, events, rsvps, people, catalog, cal_games, fingerprint: fp })
}

// ---------------------------------------------------------------------------------------------
// Snapshot (the contract both UIs render — task spec + DESIGN.md Appendix A)
// ---------------------------------------------------------------------------------------------

/// §2 / §6 `roster.presence` — the pure rule lives in games_model.rs (`model::roster`, shared
/// with the web): 10.5.1's default `open` counts everyone in the call (RSVP or not, one person
/// included) and, outside a call, the planned roster plus me.
fn roster_for(data: &RoomData, session_id: &str, present: &[String], settings: &Settings) -> Roster {
    let rsvps: &[(String, String)] = data.rsvps.get(session_id).map(Vec::as_slice).unwrap_or(&[]);
    model::roster(rsvps, present, &data.me, &data.prefs, settings.roster_presence)
}

/// Everything tonight's draw depends on, computed once per snapshot / spin.
struct Tonight {
    roster: Roster,
    credits: HashMap<String, u32>,
    pool: Pool,
    names: HashMap<String, String>,
    /// per session: which records stand / are superseded / are final
    resolved: HashMap<String, Resolved>,
    /// final spin of every session, newest first (winner = the derived one)
    finals_all: Vec<SpinRecord>,
    /// winners of the last `damping.sessions` finals of OTHER sessions
    recent_winners: Vec<String>,
}

fn tonight(data: &RoomData, session_id: &str, present: &[String], settings: &Settings) -> Tonight {
    let roster = roster_for(data, session_id, present, settings);
    let names: HashMap<String, String> = data.people.iter().map(|(k, v)| (k.clone(), v.name.clone())).collect();

    let mut by_session: HashMap<String, Vec<SpinRecord>> = HashMap::new();
    for s in &data.spins {
        by_session.entry(s.session_id.clone()).or_default().push(s.clone());
    }
    let mut resolved: HashMap<String, Resolved> = HashMap::new();
    let mut finals_all: Vec<SpinRecord> = Vec::new();
    for (sid, recs) in &by_session {
        let r = model::resolve_session(recs);
        if let Some(fid) = &r.final_id {
            if let Some(f) = recs.iter().find(|x| &x.id == fid) {
                // A record whose seed we fetched and which disagreed with it never counts toward
                // anyone's regret credit or the recent-winner damping.
                if !f.tampered {
                    finals_all.push(f.clone());
                }
            }
        }
        resolved.insert(sid.clone(), r);
    }
    finals_all.sort_by(|a, b| b.ts.cmp(&a.ts).then_with(|| b.id.cmp(&a.id)));
    // Tonight's credit comes from history: the finals of OTHER sessions (a respin after a veto
    // keeps the same credits the first spin had).
    let finals_past: Vec<SpinRecord> = finals_all.iter().filter(|s| s.session_id != session_id).cloned().collect();
    let recent_winners = model::recent_winners(&finals_past, settings);

    // 10.5.2 free spins: banked credit is neither earned, spent nor USED — tonight's draw weighs
    // nobody up (no "Lost last time, so it's weighted up", no +N badges).
    let credits = model::tonight_credits(&roster.actual, &data.prefs, &finals_past, &data.adjustments, settings);
    let pool = model::build_pool(&roster.actual, &data.prefs, settings, &names);
    Tonight { roster, credits, pool, names, resolved, finals_all, recent_winners }
}

/// The weighted spinnable set minus `vetoed`, and the reel = its top `reel.max_items` by
/// weight (record order = weight desc, id asc — what the strip scrolls and the seed walks).
fn draw(t: &Tonight, data: &RoomData, settings: &Settings, vetoed: &[String]) -> (Vec<GameWeight>, Vec<(String, f64)>) {
    let ids: Vec<String> = t.pool.spinnable(settings).into_iter().filter(|g| !vetoed.contains(g)).collect();
    let mut ws = model::weights_over(&ids, &t.pool, &t.roster.actual, &data.prefs, &t.credits, &t.recent_winners, settings);
    ws.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.id.cmp(&b.id)));
    let reel: Vec<(String, f64)> = ws
        .iter()
        .filter(|w| w.weight > 0.0)
        .take(settings.reel_max_items.max(1) as usize)
        .map(|w| (w.id.clone(), model::round6(w.weight)))
        .collect();
    (ws, reel)
}

pub(crate) fn art_url(base: &str, id: &str, kind: &str) -> String {
    format!("{base}/art/{}/{kind}", enc(id))
}

/// A readable name for a game the catalog has not answered for yet.
fn fallback_name(id: &str) -> String {
    if let Some(slug) = id.strip_prefix("custom:") {
        let mut out = String::new();
        for word in slug.split(['-', '_']) {
            let mut cs = word.chars();
            if let Some(f) = cs.next() {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(&f.to_uppercase().to_string());
                out.push_str(cs.as_str());
            }
        }
        if !out.is_empty() {
            return out;
        }
    }
    if let Some(app) = id.strip_prefix("steam:") {
        return format!("Steam app {app}");
    }
    id.to_string()
}

fn game_name(data: &RoomData, id: &str) -> String {
    data.catalog.get(id).filter(|c| c.ok && !c.name.is_empty()).map(|c| c.name.clone()).unwrap_or_else(|| fallback_name(id))
}

/// The calendar's curated entry for a catalog id (by `catalogId`, else by name) — colour +
/// server for the result panel (§4.2 "join info").
fn cal_game_for<'a>(data: &'a RoomData, id: &str, name: &str) -> Option<&'a Value> {
    let lname = name.to_lowercase();
    data.cal_games
        .iter()
        .find(|g| g.get("catalogId").and_then(Value::as_str) == Some(id))
        .or_else(|| data.cal_games.iter().find(|g| g.get("name").and_then(Value::as_str).map(|n| n.to_lowercase() == lname).unwrap_or(false)))
}

#[allow(clippy::too_many_arguments)]
fn game_json(data: &RoomData, base: &str, id: &str, tier: &str, weight: f64, odds: f64, in_reel: bool, explain: &[String], badges: &[model::Badge], vis: OddsVisibility, owners: &[String], missing: &[String], reason: Option<&str>) -> Value {
    let name = game_name(data, id);
    let (weight, odds) = if vis == OddsVisibility::Full { (weight, odds) } else { (0.0, 0.0) };
    let badges: Vec<Value> = if vis == OddsVisibility::Hidden {
        Vec::new()
    } else {
        badges.iter().map(|b| json!({ "kind": b.kind, "text": b.text })).collect()
    };
    let mut v = json!({
        "id": id,
        "name": name,
        "year": data.catalog.get(id).map(|c| c.year).unwrap_or(0),
        "art": { "portrait": art_url(base, id, "portrait"), "header": art_url(base, id, "header") },
        "tier": tier,
        "weight": model::round6(weight),
        "odds": model::round6(odds),
        "inReel": in_reel,
        "badges": badges,
        "explain": explain,
        "owners": owners,
        "missing": missing,
    });
    if let Some(r) = reason {
        v["reason"] = json!(r);
    }
    if let Some(cg) = cal_game_for(data, id, &name) {
        v["color"] = cg.get("color").cloned().unwrap_or(Value::Null);
        v["server"] = cg.get("server").cloned().unwrap_or(Value::Null);
    }
    v
}

struct Computed {
    json: Value,
    my_prefs: Value,
    is_admin: bool,
    /// the session's standing (final) spin, if any
    standing: Option<SpinRecord>,
    roster_actual: Vec<String>,
    /// (gameId, name) the calendar event still lacks after the standing spin
    writeback: Option<(String, String)>,
    writeback_due: u64,
    can_spin: bool,
    can_veto: bool,
    respins_left: u32,
    notice: String,
    /// the session this snapshot is about (the picked calendar event or tonight's ad-hoc id)
    session_id: String,
}

fn compute(data: &RoomData, base: &str, session_id: &str, present: &[String], settings: &Settings, now: u64) -> Computed {
    // 10.5.1: no calendar session tonight is no longer "Pick a session first." — the reel works on
    // tonight's ad-hoc session (`night-YYYY-MM-DD`), the same id on every client.
    let session_id = model::effective_session(session_id, data.events.contains_key(session_id), now);
    let session_id = session_id.as_str();
    let adhoc = model::is_adhoc_session(session_id);
    let me = data.me.as_str();
    let is_admin = data.my_pl >= settings.admin_min_pl;
    let t = tonight(data, session_id, present, settings);
    let names = &t.names;
    let name_of = |m: &str| model::display_name(names, m);

    // ---- the session's records: standing chain, race losers, final ----
    let resolved = t.resolved.get(session_id).cloned().unwrap_or_default();
    let session_records: Vec<&SpinRecord> = data.spins.iter().filter(|s| s.session_id == session_id).collect();
    let standing: Option<SpinRecord> = resolved.final_id.as_ref().and_then(|id| session_records.iter().find(|s| &s.id == id).map(|s| (*s).clone()));
    let latest_voided: Option<&SpinRecord> = if standing.is_none() {
        session_records.iter().filter(|s| s.voided.is_some()).max_by_key(|s| s.ts).copied()
    } else {
        None
    };
    let vetoed: Vec<String> = standing.as_ref().map(|s| s.vetoed.clone()).unwrap_or_default();
    let respins_used = model::respins_used_in(&resolved.chain, &session_records);
    let respins_left = settings.veto_respin_limit.saturating_sub(respins_used);
    let my_vetoes = standing.as_ref().map(|s| s.veto_by.values().filter(|v| v.as_str() == me).count() as u32).unwrap_or(0);

    // ---- tiers + odds ----
    let (ws, reel) = draw(&t, data, settings, &vetoed);
    let odds = model::odds(&ws);
    let vis = if is_admin { OddsVisibility::Full } else { settings.odds_visibility };
    let spinnable: Vec<String> = ws.iter().map(|w| w.id.clone()).collect();
    let ctx = ExplainCtx { pool: &t.pool, present: &t.roster.actual, prefs: &data.prefs, credits: &t.credits, names, settings, spinnable: &spinnable };
    let empty: Vec<String> = Vec::new();
    let entry = |id: &str, tier: &str, reason: Option<&str>| -> Value {
        let wi = ws.iter().position(|w| w.id == id);
        let w = wi.map(|i| &ws[i]);
        let o = wi.map(|i| odds[i]).unwrap_or(0.0);
        let (explain, badges) = model::explain(&ctx, id, w);
        let in_reel = reel.iter().any(|(g, _)| g == id);
        game_json(
            data,
            base,
            id,
            tier,
            w.map(|w| w.weight).unwrap_or(0.0),
            o,
            in_reel,
            &explain,
            &badges,
            vis,
            t.pool.owners.get(id).unwrap_or(&empty),
            t.pool.missing.get(id).unwrap_or(&empty),
            reason,
        )
    };
    // Main tier in reel order (weight desc) so the strip and the list agree; vetoed games of
    // the standing spin are listed last with the veto as their reason.
    let mut main: Vec<Value> = ws.iter().filter(|w| t.pool.main.contains(&w.id)).map(|w| entry(&w.id, "main", None)).collect();
    let mut stretch: Vec<Value> = t.pool.stretch.iter().filter(|g| !vetoed.contains(g)).map(|g| entry(g, "stretch", None)).collect();
    if settings.pool_spin_stretch {
        // Spinnable stretch games are sorted by weight too.
        stretch = ws.iter().filter(|w| t.pool.stretch.contains(&w.id)).map(|w| entry(&w.id, "stretch", None)).collect();
    }
    let mut removed: Vec<Value> = t.pool.removed.iter().map(|(g, r)| entry(g, "removed", Some(r.as_str()))).collect();
    for g in &vetoed {
        if t.pool.main.contains(g) || t.pool.stretch.contains(g) {
            let by = standing.as_ref().and_then(|s| s.veto_by.get(g)).map(|m| name_of(m)).unwrap_or_else(|| "Someone".to_string());
            let reason = format!("{by} vetoed it tonight.");
            let mut v = entry(g, "removed", Some(reason.as_str()));
            v["explain"] = json!([reason]);
            removed.push(v);
        }
    }
    main.retain(|v| !vetoed.iter().any(|g| v["id"] == g.as_str()));

    // ---- can I spin / veto? (§4.2 controls, §11 edge cases) ----
    let here = !t.roster.actual.is_empty();
    let im_here = t.roster.actual.iter().any(|p| p == me);
    let session_known = adhoc || data.events.contains_key(session_id);
    let nothing_listed = t.pool.main.is_empty() && t.pool.stretch.is_empty() && t.pool.removed.is_empty();
    let only_option: Option<String> = if reel.len() == 1 && settings.reel_min_items > 1 { Some(reel[0].0.clone()) } else { None };
    let mut notice = String::new();
    if !settings.feature_enabled {
        notice = "Game night is switched off.".into();
    } else if !session_known {
        notice = "Pick a session first.".into();
    } else if !here {
        notice = "Nobody's here yet.".into();
    } else if reel.is_empty() && nothing_listed {
        // 10.5.1: say what's missing instead of "No game everyone here owns."
        notice = "Nobody here has listed any games yet — add yours in My games.".into();
    } else if reel.is_empty() {
        notice = if t.pool.main.is_empty() && !t.pool.stretch.is_empty() {
            "No game everyone here owns — try the stretch tier.".into()
        } else {
            "No game everyone here owns.".into()
        };
    } else if (reel.len() as u32) < settings.reel_min_items {
        notice = "Only one option tonight.".into();
    } else if !im_here {
        // Unreachable with the default `open` roster (the viewer always counts); kept for the
        // stricter modes a space can still pick.
        notice = match settings.roster_presence {
            Presence::Open | Presence::CallOnly => "Join the call to spin.".into(),
            Presence::RsvpPresent | Presence::RsvpOnly => "RSVP to the session to spin.".into(),
        };
    } else if !data.can_write {
        notice = "Ask a space admin to open the flyout once — that unlocks spinning for everyone.".into();
    }
    let can_spin = notice.is_empty() && standing.is_none();
    let can_veto = settings.feature_enabled
        && data.can_write
        && im_here
        && standing.as_ref().map(|s| s.effective_winner().is_some()).unwrap_or(false)
        && my_vetoes < settings.veto_post_spin_per_person
        && respins_left > 0;

    // ---- the spin object ----
    let race_note = resolved.chain.first().and_then(|first| {
        if resolved.superseded.is_empty() {
            return None;
        }
        let by = session_records.iter().find(|s| &s.id == first).map(|s| name_of(&s.sender))?;
        Some(model::explain_superseded(&by))
    });
    let my_spin_lost = resolved.superseded.keys().any(|id| session_records.iter().any(|s| &s.id == id && s.sender == me));
    let strip_of = |s: &SpinRecord| -> Vec<Value> {
        let total: f64 = s.pool.iter().map(|p| p.1).sum();
        s.pool
            .iter()
            .map(|(id, w)| {
                json!({
                    "id": id,
                    "name": game_name(data, id),
                    "art": { "portrait": art_url(base, id, "portrait"), "header": art_url(base, id, "header") },
                    "weight": w,
                    "odds": if total > 0.0 { model::round6(w / total) } else { 0.0 },
                })
            })
            .collect()
    };
    let spin_json = match (&standing, latest_voided) {
        (Some(s), _) => {
            let winner = s.effective_winner().map(|w| w.to_string());
            let winner_name = winner.as_deref().map(|w| game_name(data, w)).unwrap_or_default();
            let owners: Vec<String> = winner.as_deref().and_then(|w| t.pool.owners.get(w)).cloned().unwrap_or_default();
            let mut v = json!({
                "id": s.id,
                "seedEventId": s.seed_id(),
                "winner": winner,
                "winnerName": winner_name,
                "claimedWinner": s.winner,
                "kind": s.kind,
                "vetoed": s.vetoed,
                "vetoBy": s.veto_by,
                "by": s.sender,
                "byName": name_of(&s.sender),
                "at": s.ts,
                "verified": s.verified,
                "superseded": false,
                "voided": false,
                "pool": strip_of(s),
                "roster": s.roster,
                "late": s.late,
                "owners": owners,
                "respinsUsed": respins_used,
                "mySpinLost": my_spin_lost,
                "settingsRev": s.settings_rev,
                // 10.5.2: false = this spin changes nobody's credit (free spin / "Don't count for credit")
                "credit": s.credit,
            });
            if let Some(n) = &race_note {
                v["note"] = json!(n);
            }
            if let Some(w) = &winner {
                if let Some(cg) = cal_game_for(data, w, &game_name(data, w)) {
                    v["server"] = cg.get("server").cloned().unwrap_or(Value::Null);
                    v["color"] = cg.get("color").cloned().unwrap_or(Value::Null);
                }
            }
            v
        }
        (None, Some(s)) => {
            let vd = s.voided.clone().unwrap_or_default();
            json!({
                "id": s.id,
                "seedEventId": s.seed_id(),
                "winner": s.effective_winner(),
                "winnerName": s.effective_winner().map(|w| game_name(data, w)).unwrap_or_default(),
                "claimedWinner": s.winner,
                "kind": s.kind,
                "vetoed": s.vetoed,
                "vetoBy": s.veto_by,
                "by": s.sender,
                "byName": name_of(&s.sender),
                "at": s.ts,
                "verified": s.verified,
                "superseded": false,
                "voided": true,
                "voidedBy": vd.by,
                "voidReason": vd.reason,
                "note": model::explain_voided(&name_of(&vd.by), &vd.reason),
                "pool": strip_of(s),
                "roster": s.roster,
                "late": s.late,
                "respinsUsed": respins_used,
                "mySpinLost": my_spin_lost,
                "credit": s.credit,
            })
        }
        (None, None) => Value::Null,
    };

    // ---- calendar write-back state (§7.2) ----
    let session_ev = data.events.get(session_id);
    let mut writeback: Option<(String, String)> = None;
    let mut writeback_due = 0u64;
    if let (Some(s), Some(ev)) = (&standing, session_ev) {
        // Never publish a tampered record's winner to the shared calendar.
        if let Some(w) = s.effective_winner().filter(|_| !s.tampered) {
            if ev.get("gameId").and_then(Value::as_str) != Some(w) {
                writeback = Some((w.to_string(), game_name(data, w)));
                writeback_due = s.ts.saturating_add(WRITEBACK_FALLBACK_MS);
            }
        }
    }

    // ---- log (§4.2): every session with spins, newest first ----
    let mut sessions: Vec<(&String, &Resolved)> = t.resolved.iter().collect();
    let latest_ts = |sid: &str| data.spins.iter().filter(|s| s.session_id == sid).map(|s| s.ts).max().unwrap_or(0);
    sessions.sort_by(|a, b| latest_ts(b.0).cmp(&latest_ts(a.0)));
    let mut log: Vec<Value> = Vec::new();
    for (sid, res) in sessions.into_iter().take(LOG_LIMIT) {
        let recs: Vec<&SpinRecord> = data.spins.iter().filter(|s| &s.session_id == sid).collect();
        let fin = res.final_id.as_ref().and_then(|id| recs.iter().find(|s| &s.id == id).copied());
        let latest = recs.iter().max_by_key(|s| s.ts).copied();
        let Some(shown) = fin.or(latest) else { continue };
        let ev = data.events.get(sid);
        let title = ev.and_then(|e| e.get("title")).and_then(Value::as_str).unwrap_or("").to_string();
        let date = ev.and_then(|e| e.get("start")).and_then(Value::as_u64).filter(|s| *s > 0).unwrap_or(shown.ts);
        let winner = shown.effective_winner().map(|w| w.to_string());
        let mut credit_changes: Vec<Value> = Vec::new();
        if let Some(f) = fin {
            if let Some(i) = t.finals_all.iter().position(|x| x.id == f.id) {
                for p in &f.roster {
                    if f.flexible.contains(p) {
                        continue;
                    }
                    let Some(fav) = f.favorites.get(p) else { continue };
                    if !f.in_pool(fav) {
                        continue;
                    }
                    let then = Prefs { favorites: vec![fav.clone()], ..Default::default() };
                    let before = model::derive_credit(p, &t.finals_all[i + 1..], &then, settings, None) as i64;
                    let after = model::derive_credit(p, &t.finals_all[i..], &then, settings, None) as i64;
                    if after != before {
                        credit_changes.push(json!({ "mxid": p, "name": name_of(p), "delta": after - before, "game": fav, "gameName": game_name(data, fav) }));
                    }
                }
            }
        }
        let voided = fin.is_none() && shown.voided.is_some();
        let mut entry = json!({
            "sessionId": sid,
            "title": title,
            "date": date,
            "spinId": shown.id,
            "at": shown.ts,
            "winner": winner,
            "winnerName": winner.as_deref().map(|w| game_name(data, w)).unwrap_or_default(),
            "by": shown.sender,
            "byName": name_of(&shown.sender),
            "kind": shown.kind,
            "vetoed": shown.vetoed,
            "present": shown.roster,
            "presentNames": shown.roster.iter().map(|p| name_of(p)).collect::<Vec<_>>(),
            "creditChanges": credit_changes,
            "verified": shown.verified,
            "voided": voided,
            "current": sid == session_id,
            // 10.5.2: the shown record counts for credit (false: a free spin / "Don't count for credit")
            "credit": shown.credit,
        });
        if let Some(v) = &shown.voided {
            entry["voidReason"] = json!(v.reason);
            entry["voidedBy"] = json!(v.by);
            entry["note"] = json!(model::explain_voided(&name_of(&v.by), &v.reason));
        }
        log.push(entry);
    }

    // ---- 10.5.2: everyone's banked credit (Tune's "Credit" list) — free mode doesn't change it. As of
    // NOW, tonight's result included (tonight's draw weighs with the credit from before it), so an
    // admin sees what tonight's spin did and can undo it ----
    let mut known: HashSet<String> = data.prefs.keys().cloned().collect();
    known.extend(data.adjustments.keys().cloned());
    for s in &data.spins {
        known.extend(s.roster.iter().cloned());
    }
    let no_prefs = Prefs::default();
    let mut credit_rows: Vec<(String, String, Value)> = known
        .iter()
        .filter(|m| m.starts_with('@'))
        .map(|m| {
            let pr = data.prefs.get(m).unwrap_or(&no_prefs);
            let adj = data.adjustments.get(m);
            let d = model::credit_detail(m, &t.finals_all, pr, settings, adj);
            let offset = adj.filter(|_| d.offset_live).map(|a| a.set.unwrap_or(0) + a.delta).unwrap_or(0);
            let name = name_of(m);
            let row = json!({ "mxid": m, "name": name, "credit": d.credit, "offset": offset, "hasFavorite": pr.favorite().is_some() });
            (name.to_lowercase(), m.clone(), row)
        })
        .collect();
    credit_rows.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let credit_rows: Vec<Value> = credit_rows.into_iter().map(|(_, _, r)| r).collect();

    // ---- roster ----
    let person = |m: &String| -> Value {
        let p = data.people.get(m);
        json!({ "mxid": m, "name": name_of(m), "avatar": p.map(|p| p.avatar.as_str()).unwrap_or(""), "flexible": t.roster.flexible.contains(m), "credit": t.credits.get(m).copied().unwrap_or(0), "hasPrefs": data.prefs.contains_key(m) })
    };
    let adhoc_game = if adhoc { standing.as_ref().and_then(|s| s.effective_winner()).map(|w| (w.to_string(), game_name(data, w))) } else { None };
    let session_json = if adhoc {
        // Tonight without a calendar event: nothing to write back; the header shows the pick.
        json!({
            "id": session_id,
            "title": "",
            "start": 0,
            "end": 0,
            "kind": "adhoc",
            "adhoc": true,
            "game": adhoc_game.as_ref().map(|g| g.1.as_str()).unwrap_or(""),
            "gameId": adhoc_game.as_ref().map(|g| g.0.as_str()).unwrap_or(""),
        })
    } else {
        session_ev.map(|e| {
            json!({
                "id": session_id,
                "title": e.get("title").and_then(Value::as_str).unwrap_or(""),
                "start": e.get("start").and_then(Value::as_u64).unwrap_or(0),
                "end": e.get("end").and_then(Value::as_u64).unwrap_or(0),
                "kind": e.get("kind").and_then(Value::as_str).unwrap_or("session"),
                "game": e.get("game").and_then(Value::as_str).unwrap_or(""),
                "gameId": e.get("gameId").and_then(Value::as_str).unwrap_or(""),
            })
        })
        .unwrap_or(Value::Null)
    };
    let in_call = present.iter().any(|p| p.starts_with('@'));
    let live: Vec<String> = if settings.roster_presence == Presence::Open && !in_call {
        t.roster.actual.iter().filter(|p| p.as_str() == me).cloned().collect()
    } else {
        t.roster.actual.clone()
    };
    let names_json: Value = names.iter().map(|(k, v)| (k.clone(), Value::from(v.as_str()))).collect::<serde_json::Map<String, Value>>().into();
    let json = json!({
        "session": session_json,
        "roster": {
            "expected": t.roster.expected.iter().map(person).collect::<Vec<_>>(),
            "present": t.roster.actual.iter().map(person).collect::<Vec<_>>(),
            "flexible": t.roster.flexible,
            "maybes": t.roster.maybes,
            "here": t.roster.actual.len(),
            "expectedCount": t.roster.expected.len(),
            "presence": settings.roster_presence.as_str(),
            // 10.5.1: who is actually HERE — the header's "N here" + solid avatars. The open roster
            // outside a call is a preview (the RSVP'd + me), so only the viewer is here then.
            "live": live,
            "inCall": in_call,
        },
        "tiers": { "main": main, "stretch": stretch, "removed": removed },
        "reel": reel.iter().map(|(id, _)| id).collect::<Vec<_>>(),
        "spin": spin_json,
        "canSpin": can_spin,
        "canVeto": can_veto,
        "respinsLeft": respins_left,
        "myVetoesLeft": settings.veto_post_spin_per_person.saturating_sub(my_vetoes),
        "notice": notice,
        "onlyOption": only_option,
        "log": log,
        "names": names_json,
        "me": me,
        "isAdmin": is_admin,
        "featureEnabled": settings.feature_enabled,
        "oddsVisibility": match vis { OddsVisibility::Full => "full", OddsVisibility::BadgesOnly => "badges-only", OddsVisibility::Hidden => "hidden" },
        "reelSettings": { "durationMs": settings.reel_duration_ms, "laps": settings.reel_laps, "minItems": settings.reel_min_items, "maxItems": settings.reel_max_items, "spinStretch": settings.pool_spin_stretch },
        "settingsRev": settings.rev,
        "calendarWritten": writeback.is_none() && standing.is_some(),
        "computedAt": now,
        // 10.5.2 settings credits.mode — "free": spins don't earn, spend or use credit
        "creditsMode": settings.credits_mode.as_str(),
        // 10.5.2: everyone's banked credit (Tune's "Credit" list), by name
        "credits": credit_rows,
    });
    let my_prefs = data.prefs.get(me).map(Prefs::to_json).unwrap_or_else(|| json!({}));
    Computed { json, my_prefs, is_admin, standing, roster_actual: t.roster.actual.clone(), writeback, writeback_due, can_spin, can_veto, respins_left, notice, session_id: session_id.to_string() }
}

// ---------------------------------------------------------------------------------------------
// Writes
// ---------------------------------------------------------------------------------------------

/// `send_state` that hands back the new event id (a spin's first write is its seed).
async fn send_state_id(room: &Room, event_type: &str, state_key: &str, content: Value) -> Result<String, String> {
    room.send_state_event_raw(event_type, state_key, content)
        .await
        .map(|r| r.event_id.to_string())
        .map_err(|e| friendly(&e.to_string()))
}

/// 10.5.2 review: wait (≤ 5 s) until the LOCAL store holds the state event we just wrote. The send
/// returns before the one sync loop delivers it, so the re-read after an admin credit write could
/// still see the OLD adjustment — busy cleared, and a quick second "+" (or a "+" right after "Zero")
/// started from that stale value and overwrote the first write. A poll, like
/// `appstate::provision_new_room` (never a second sync). Bounded: another admin's newer write simply
/// times out. The web's gamesStore `waitForEcho` is the twin.
async fn wait_for_echo(room: &Room, event_type: &str, state_key: &str, event_id: &str) {
    if event_id.is_empty() {
        return;
    }
    for _ in 0..100 {
        if get_state_full(room, event_type).await.iter().any(|e| e["state_key"] == state_key && e["event_id"] == event_id) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

async fn do_save_prefs(cid: &str, raw: &str) -> Result<(), String> {
    let v: Value = serde_json::from_str(raw).map_err(|_| "Bad prefs data".to_string())?;
    let client = crate::session::client().ok_or("Not signed in")?;
    let me = client.user_id().map(|u| u.to_string()).ok_or("Not signed in")?;
    let room = container_room(cid).ok_or("Not signed in")?;
    crate::appstate::ensure_writable(&room, &[PREFS_TYPE, SPIN_TYPE]).await;
    let settings = crate::appstate::get_state(&room, SETTINGS_TYPE, "").await.map(|s| Settings::from_json(&s)).unwrap_or_default();
    let mut prefs = Prefs::from_json(&v).normalized(settings.favorite_slots);
    prefs.updated_at = crate::appstate::now_ms();
    let content = if prefs.is_empty() { json!({}) } else { prefs.to_json() };
    send_state_id(&room, PREFS_TYPE, &me, content).await?;
    eprintln!("[games] prefs saved: {} would-play, favorite {:?}, flexible {}", prefs.would_play.len(), prefs.favorite(), prefs.flexible);
    Ok(())
}

/// §8 steps 1–3 + 6. A veto / respin is the same draw over the remaining games with
/// `supersedes` pointing at the record it replaces (§7.2).
async fn do_spin(cid: &str, base: &str, session_id: &str, present: &[String], mode: SpinMode) -> Result<(), String> {
    let client = crate::session::client().ok_or("Not signed in")?;
    let room = container_room(cid).ok_or("Not signed in")?;
    crate::appstate::ensure_writable(&room, &[PREFS_TYPE, SPIN_TYPE]).await;
    // A fresh read: a spin that landed a moment ago must be seen before we add another.
    let data = load_room_data(&client, cid, base, session_id, present).await.ok_or("Couldn't read the space")?;
    let settings = data.settings.clone();
    let now = crate::appstate::now_ms();
    let c = compute(&data, base, session_id, present, &settings, now);
    // 10.5.1: the session compute resolved (tonight's ad-hoc id when no calendar session is picked).
    let session_id = c.session_id.clone();
    let session_id = session_id.as_str();
    let me = data.me.clone();
    let t = tonight(&data, session_id, present, &settings);
    // 10.5.1 (the web's fix, TW docs/native-bugs/calendar.md): a veto / re-spin chains onto the
    // chain's TAIL, voided or not. Pointing it at the STANDING record made every re-spin after an
    // admin voided the latest one a sibling of that voided record — which (older) won the race,
    // so the session stayed stuck on the old spin with a bogus "Two people spun at once" note.
    let chain_tail = |fallback: &str| -> String {
        t.resolved.get(session_id).and_then(|r| r.chain.last().cloned()).unwrap_or_else(|| fallback.to_string())
    };

    let (kind, supersedes, vetoed, veto_by) = match mode {
        SpinMode::Initial => {
            if c.standing.is_some() {
                return Err("Someone already spun — the result is on its way.".into());
            }
            if !c.can_spin {
                return Err(if c.notice.is_empty() { "Can't spin right now.".into() } else { c.notice.clone() });
            }
            // 10.5.1: after an admin voided tonight's spin ("Void tonight's spin" exists so the group
            // can spin again) a new initial spin with no `supersedes` was a sibling of the voided
            // record and lost the race to it — nothing ever stood again. It chains onto the tail.
            let after_void = t.resolved.get(session_id).and_then(|r| r.chain.last().cloned());
            ("initial".to_string(), after_void, Vec::new(), HashMap::new())
        }
        SpinMode::Veto => {
            let st = c.standing.clone().ok_or("Nothing to veto yet.")?;
            if !c.can_veto {
                return Err("You can't veto this one.".into());
            }
            let w = st.effective_winner().ok_or("No result to veto.")?.to_string();
            let mut vetoed = st.vetoed.clone();
            if !vetoed.contains(&w) {
                vetoed.push(w.clone());
            }
            let mut veto_by = st.veto_by.clone();
            veto_by.insert(w, me.clone());
            ("respin".to_string(), Some(chain_tail(&st.id)), vetoed, veto_by)
        }
        SpinMode::Respin => {
            let st = c.standing.clone().ok_or("Nothing to re-spin yet.")?;
            if !settings.feature_enabled {
                return Err("Game night is switched off.".into());
            }
            if c.respins_left == 0 {
                return Err("No re-spins left tonight.".into());
            }
            if !t.roster.actual.iter().any(|p| p == &me) {
                return Err("You're not on tonight's roster.".into());
            }
            ("respin".to_string(), Some(chain_tail(&st.id)), st.vetoed.clone(), st.veto_by.clone())
        }
    };
    let (_, reel) = draw(&t, &data, &settings, &vetoed);
    if reel.is_empty() {
        return Err("No game left to spin — everything was vetoed.".into());
    }
    let favorites: HashMap<String, String> = t
        .roster
        .actual
        .iter()
        .filter(|p| !t.roster.flexible.contains(p))
        .filter_map(|p| data.prefs.get(p).and_then(|x| x.favorite()).map(|f| (p.clone(), f.to_string())))
        .collect();
    let mut rec = SpinRecord {
        id: format!("sp-{}-{:04x}", now, rand::random::<u16>()),
        session_id: session_id.to_string(),
        kind,
        supersedes,
        vetoed,
        veto_by,
        roster: t.roster.actual.clone(),
        maybes: t.roster.maybes.clone(),
        flexible: t.roster.flexible.clone(),
        favorites,
        // 10.5.1: integer micro-units — Synapse refuses JSON floats in room-v10 state, which made
        // every 10.5.0 spin fail. Built BEFORE write 1 so the winner below is derived from exactly
        // the numbers every reader reads (readers are scale-free: 10.5.0, 10.5.1 and the web agree).
        pool: reel.iter().map(|(id, w)| (id.clone(), model::to_micro(*w))).collect(),
        settings_rev: settings.rev,
        // 10.5.2 free spins: captured at write time (write 1 = the seed, so it can't be flipped later
        // by anyone but an admin) — changing credits.mode never rewrites history
        credit: settings.credits_mode != model::CreditsMode::Free,
        ..Default::default()
    };
    // Write 1: the record without a winner. Its event id is the seed nobody controls.
    let seed = send_state_id(&room, SPIN_TYPE, &rec.id, rec.to_json()).await?;
    let winner = model::derive_winner(&seed, &rec.pool).ok_or("The draw came up empty")?;
    // Write 2: the same record + seed + winner (a state REPLACE, hence `seed` travels along).
    rec.seed = seed.clone();
    rec.winner = Some(winner.clone());
    if let Err(e) = send_state_id(&room, SPIN_TYPE, &rec.id, rec.to_json()).await {
        // Every other client derives the winner from the first write regardless (§11).
        eprintln!("[games] winner write failed after seed {seed}: {e}");
    }
    eprintln!("[games] spin {} ({}) in {session_id}: seed {seed} → {winner} over {} games", rec.id, rec.kind, rec.pool.len());
    write_back_calendar(&room, session_id, &winner, &game_name(&data, &winner)).await;
    Ok(())
}

/// §7.2 / §8 step 6: the session event gets `game` (catalog name) + `gameId`. Written directly
/// in `dev.tinhat.calendar.event`'s shape (every other field preserved); idempotent, so the
/// 30 s fallback from any client is harmless. The chat card is NOT re-synced here (that path
/// lives inside calendar::do_save_event and needs the pre-formatted time from QML).
async fn write_back_calendar(room: &Room, session_id: &str, game_id: &str, name: &str) -> bool {
    let Some(mut ev) = crate::appstate::get_state(room, calendar::EVENT_TYPE, session_id).await else {
        return false;
    };
    if ev.get("title").and_then(Value::as_str).map(|t| t.trim().is_empty()).unwrap_or(true) {
        return false; // deleted
    }
    if ev.get("gameId").and_then(Value::as_str) == Some(game_id) {
        return true;
    }
    ev["game"] = json!(name);
    ev["gameId"] = json!(game_id);
    ev["updatedAt"] = json!(crate::appstate::now_ms());
    match crate::appstate::send_state(room, calendar::EVENT_TYPE, session_id, ev).await {
        Ok(()) => {
            eprintln!("[games] calendar {session_id} set to {name} ({game_id})");
            true
        }
        Err(e) => {
            eprintln!("[games] calendar write-back failed: {e}");
            false
        }
    }
}

/// `roster.grace_min`: the spinner appends people who arrived within the window. Pool and
/// weights are untouched; `late` marks them so verification still matches the seed.
async fn do_late_arrivals(cid: &str, spin_id: &str, newcomers: &[String]) -> Result<(), String> {
    let room = container_room(cid).ok_or("Not signed in")?;
    let raw = get_state_full(&room, SPIN_TYPE).await.into_iter().find(|e| e["state_key"] == spin_id).ok_or("Spin vanished")?;
    let mut c = raw.get("content").cloned().ok_or("Spin vanished")?;
    let cur_id = raw["event_id"].as_str().unwrap_or("").to_string();
    if c.get("seed").and_then(Value::as_str).map(|s| s.is_empty()).unwrap_or(true) {
        c["seed"] = json!(cur_id);
    }
    let mut roster: Vec<String> = c.get("roster").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(|s| s.to_string()).collect()).unwrap_or_default();
    let mut late: Vec<String> = c.get("late").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(|s| s.to_string()).collect()).unwrap_or_default();
    let mut changed = false;
    for p in newcomers {
        if !roster.contains(p) {
            roster.push(p.clone());
            late.push(p.clone());
            changed = true;
        }
    }
    if !changed {
        return Ok(());
    }
    c["roster"] = json!(roster);
    c["late"] = json!(late);
    send_state_id(&room, SPIN_TYPE, spin_id, c).await?;
    eprintln!("[games] {spin_id}: {} late arrival(s) added to the roster", newcomers.len());
    Ok(())
}

async fn do_save_settings(cid: &str, raw: &str) -> Result<(), String> {
    let patch: Value = serde_json::from_str(raw).map_err(|_| "Bad settings data".to_string())?;
    if !patch.is_object() {
        return Err("Bad settings data".into());
    }
    let client = crate::session::client().ok_or("Not signed in")?;
    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let room = container_room(cid).ok_or("Not signed in")?;
    let current = crate::appstate::get_state(&room, SETTINGS_TYPE, "").await.map(|s| Settings::from_json(&s)).unwrap_or_default();
    let mut next = current.merged(&patch);
    next.rev = current.rev + 1;
    // 10.5.1: the float-free wire form (10.5.0 wrote `0.5` / `0.0` and Synapse refused every save).
    let mut content = next.to_wire()?;
    content["updatedBy"] = json!(me);
    content["updatedAt"] = json!(crate::appstate::now_ms());
    send_state_id(&room, SETTINGS_TYPE, "", content).await?;
    eprintln!("[games] settings rev {} saved by {me}", next.rev);
    Ok(())
}

/// 10.5.2 admin ±N on one person's credit (Tune → Credit; the web's gamesStore `adjustCredit`):
/// written at "admin:<mxid>" — the target's own @-key is refused by Matrix for anyone but them —
/// starting from the adjustment that counts now (`model::adjust_content`: a live offset carries
/// forward, a lapsed one starts over). A fresh read, like a spin.
async fn do_adjust(cid: &str, base: &str, session_id: &str, present: &[String], mxid: &str, delta: i64, reason: &str) -> Result<(), String> {
    let client = crate::session::client().ok_or("Not signed in")?;
    let room = container_room(cid).ok_or("Not signed in")?;
    let data = load_room_data(&client, cid, base, session_id, present).await.ok_or("Couldn't read the space")?;
    let now = crate::appstate::now_ms();
    let sid = model::effective_session(session_id, data.events.contains_key(session_id), now);
    // every result so far, TONIGHT'S TOO: a win tonight has already spent an old offset, so a "+1"
    // given after it must not carry that offset forward (the Tune list shows the same)
    let finals = tonight(&data, &sid, present, &data.settings).finals_all;
    let cur = data.adjustments.get(mxid);
    let no_prefs = Prefs::default();
    let live = model::credit_detail(mxid, &finals, data.prefs.get(mxid).unwrap_or(&no_prefs), &data.settings, cur).offset_live;
    let key = model::admin_adjust_key(mxid);
    let eid = send_state_id(&room, ADJUST_TYPE, &key, model::adjust_content(cur, live, delta, reason, &data.me, now)).await?;
    wait_for_echo(&room, ADJUST_TYPE, &key, &eid).await;
    eprintln!("[games] credit of {mxid} adjusted by {delta:+} ({reason}) by {}", data.me);
    Ok(())
}

/// 10.5.2: one person's credit back to 0 as of now (Tune → Credit → Zero), at "admin:<mxid>".
async fn do_zero_credit(cid: &str, mxid: &str) -> Result<(), String> {
    let client = crate::session::client().ok_or("Not signed in")?;
    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let room = container_room(cid).ok_or("Not signed in")?;
    let key = model::admin_adjust_key(mxid);
    let eid = send_state_id(&room, ADJUST_TYPE, &key, model::zero_content("Reset", &me, crate::appstate::now_ms())).await?;
    wait_for_echo(&room, ADJUST_TYPE, &key, &eid).await;
    eprintln!("[games] credit of {mxid} reset by {me}");
    Ok(())
}

/// Reset-all writes one adjustment per known person with `set: 0` as of now (§7.4). 10.5.2: at
/// "admin:<mxid>" — up to 10.5.1 it went to the person's own @-key, which Matrix (room v10 auth
/// rules) refuses for anyone but them, so it stopped at the first other person and only the admin's
/// own credit was ever reset. Keeps going past a failure and says who is left (the web's words).
async fn do_reset_credits(cid: &str) -> Result<(), String> {
    let client = crate::session::client().ok_or("Not signed in")?;
    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let room = container_room(cid).ok_or("Not signed in")?;
    let now = crate::appstate::now_ms();
    let mut people: HashSet<String> = HashSet::new();
    for (sk, _, c) in crate::appstate::get_state_all(&room, PREFS_TYPE).await {
        if c.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
            people.insert(sk);
        }
    }
    for (sk, _, _) in crate::appstate::get_state_all(&room, ADJUST_TYPE).await {
        if let Some((target, _)) = model::adjust_target(&sk) {
            people.insert(target);
        }
    }
    for (_, _, c) in crate::appstate::get_state_all(&room, SPIN_TYPE).await {
        if let Some(r) = c.get("roster").and_then(Value::as_array) {
            people.extend(r.iter().filter_map(Value::as_str).map(|s| s.to_string()));
        }
    }
    let mut people: Vec<String> = people.into_iter().filter(|p| p.starts_with('@')).collect();
    people.sort();
    let mut failed: Vec<String> = Vec::new();
    let mut last: Option<(String, String)> = None;
    for p in &people {
        let key = model::admin_adjust_key(p);
        match send_state_id(&room, ADJUST_TYPE, &key, model::zero_content("Reset", &me, now)).await {
            Ok(eid) => last = Some((key, eid)),
            Err(_) => failed.push(p.clone()),
        }
    }
    // the sync delivers in order: once the last reset is local, so are the others (see wait_for_echo)
    if let Some((key, eid)) = last {
        wait_for_echo(&room, ADJUST_TYPE, &key, &eid).await;
    }
    if !failed.is_empty() {
        return Err(format!("Couldn't reset {} — try again", failed.join(", ")));
    }
    eprintln!("[games] credits reset for {} people by {me}", people.len());
    Ok(())
}

/// 10.5.2 admin "Don't count for credit" / "Count for credit" on one spin (the web's gamesStore
/// `setSpinCredit`): `dev.tinhat.games.adjust` "credit:<spin id>" `{credit, by, at}` — admin-only
/// state next to the adjustments, so no member rewrite of the record can undo it; the spin record
/// itself is not touched (it still verifies, the result stands).
async fn do_set_spin_credit(cid: &str, spin_id: &str, counts: bool) -> Result<(), String> {
    let client = crate::session::client().ok_or("Not signed in")?;
    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let room = container_room(cid).ok_or("Not signed in")?;
    let exists = get_state_full(&room, SPIN_TYPE).await.into_iter().any(|e| e["state_key"] == spin_id && e["content"].as_object().map(|o| !o.is_empty()).unwrap_or(false));
    if !exists {
        return Err("No such spin".into());
    }
    let key = model::credit_override_key(spin_id);
    let eid = send_state_id(&room, ADJUST_TYPE, &key, model::credit_override_content(counts, &me, crate::appstate::now_ms())).await?;
    wait_for_echo(&room, ADJUST_TYPE, &key, &eid).await;
    eprintln!("[games] spin {spin_id} {} ({me})", if counts { "counts for credit" } else { "no longer counts for credit" });
    Ok(())
}

/// Void = the record rewritten with `voided: {by, reason, at}`; `seed` is preserved (or set to
/// the record's current event id when the spinner never wrote it) so the derivation survives.
async fn do_void(cid: &str, spin_id: &str, reason: &str) -> Result<(), String> {
    let client = crate::session::client().ok_or("Not signed in")?;
    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let room = container_room(cid).ok_or("Not signed in")?;
    let raw = get_state_full(&room, SPIN_TYPE).await.into_iter().find(|e| e["state_key"] == spin_id).ok_or("No such spin")?;
    let mut c = raw.get("content").cloned().filter(Value::is_object).ok_or("No such spin")?;
    if c.get("seed").and_then(Value::as_str).map(|s| s.is_empty()).unwrap_or(true) {
        c["seed"] = json!(raw["event_id"].as_str().unwrap_or(""));
    }
    let now = crate::appstate::now_ms();
    c["voided"] = json!({ "by": me, "reason": reason.trim(), "at": now });
    send_state_id(&room, SPIN_TYPE, spin_id, c).await?;
    eprintln!("[games] spin {spin_id} voided by {me}: {reason}");
    Ok(())
}

/// A record without its winner younger than this is a spin still in flight (write 1 → write 2).
const LANDING_MS: u64 = 20_000;

/// JavaScript truthiness of an optional JSON value — the web's `!e.content.voided` guard, kept
/// exactly (an absent / null / false / 0 / "" `voided` counts as not voided).
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().map(|x| x != 0.0 && !x.is_nan()).unwrap_or(true),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

/// 10.5.2 admin "Reset spins" (the web's gamesStore `resetSpins`, same checks and writes): a fresh
/// read like `do_spin`; the session is the one `compute` shows (the picked calendar event or
/// tonight's ad-hoc one); refused while a spin is still landing (its winner write would overwrite
/// the void); every record of the session is rewritten per `games_model::reset_writes`, one write
/// at a time in its order. A retry after a partial failure only writes what is left.
async fn do_reset_spins(cid: &str, base: &str, session_id: &str, present: &[String]) -> Result<(), String> {
    let client = crate::session::client().ok_or("Not signed in")?;
    let room = container_room(cid).ok_or("Not signed in")?;
    let data = load_room_data(&client, cid, base, session_id, present).await.ok_or("Couldn't read the space")?;
    if data.my_pl < data.settings.admin_min_pl {
        return Err("Only a space admin can reset tonight's spins".into());
    }
    let now = crate::appstate::now_ms();
    let sid = model::effective_session(session_id, data.events.contains_key(session_id), now);
    let chain = tonight(&data, &sid, present, &data.settings).resolved.get(&sid).map(|r| r.chain.clone()).unwrap_or_default();
    let mine: Vec<Value> = get_state_full(&room, SPIN_TYPE)
        .await
        .into_iter()
        .filter(|e| e["content"].as_object().map(|o| !o.is_empty()).unwrap_or(false) && e["content"]["sessionId"].as_str() == Some(sid.as_str()))
        .collect();
    let landing = mine.iter().any(|e| {
        let c = &e["content"];
        !c.get("winner").map(Value::is_string).unwrap_or(false) && !truthy(c.get("voided")) && now.saturating_sub(e["origin_server_ts"].as_u64().unwrap_or(0)) < LANDING_MS
    });
    if landing {
        return Err("A spin is still landing — try again in a moment.".into());
    }
    let raw: Vec<model::RawSpin> = mine
        .iter()
        .map(|e| model::RawSpin {
            id: e["state_key"].as_str().unwrap_or("").to_string(),
            event_id: e["event_id"].as_str().unwrap_or("").to_string(),
            content: e["content"].clone(),
        })
        .collect();
    let writes = model::reset_writes(&chain, &raw, &data.me, now);
    for (id, content) in &writes {
        send_state_id(&room, SPIN_TYPE, id, content.clone()).await?;
    }
    eprintln!("[games] spins reset in {sid} by {}: {} write(s) over {} record(s), chain {}", data.me, writes.len(), mine.len(), chain.len());
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Catalog (§7.6) — names + art come from the server's Tin Hat API, never from the events
// ---------------------------------------------------------------------------------------------

fn catalog_cache() -> &'static Mutex<HashMap<String, CatalogEntry>> {
    static C: OnceLock<Mutex<HashMap<String, CatalogEntry>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn entry_from_json(v: &Value, now: u64) -> CatalogEntry {
    CatalogEntry {
        name: v.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string(),
        year: v.get("year").and_then(Value::as_u64).unwrap_or(0),
        fetched_at: now,
        ok: v.get("name").and_then(Value::as_str).map(|n| !n.trim().is_empty()).unwrap_or(false),
    }
}

/// A search / import / custom reply already told us the name: cache it so the snapshot has it
/// before the next refresh.
fn remember_catalog_entry(id: &str, v: &Value) {
    let e = entry_from_json(v, crate::appstate::now_ms());
    if e.ok {
        if let Ok(mut c) = catalog_cache().lock() {
            c.insert(id.to_string(), e);
        }
    }
}

/// 10.5.2: the API's client carries `TinHat/<version> (<os>)` ([`crate::proxy::http`]).
fn http() -> matrix_sdk::reqwest::Client {
    crate::proxy::http(Some(std::time::Duration::from_secs(20)))
}

/// `<base><path>`, or a plain error while the base is still unknown (not signed in yet: "/games"). A relative URL
/// used to reach reqwest and come back as "builder error: relative URL without a base".
fn catalog_url(base: &str, path: &str) -> Result<String, String> {
    if base.starts_with("https://") || base.starts_with("http://") {
        Ok(format!("{base}{path}"))
    } else {
        Err("not connected to your server yet — try again in a moment".to_string())
    }
}

async fn catalog_get(base: &str, path: &str) -> Result<Value, String> {
    catalog_get_auth(base, path, None).await
}

/// 10.1 (TH-REEL-01): `/search` sits behind `requireMatrixUser` on the catalog service, so the
/// typed-search path needs the same bearer `catalog_post` already sends.
pub(crate) async fn catalog_get_auth(base: &str, path: &str, bearer: Option<&str>) -> Result<Value, String> {
    let url = catalog_url(base, path)?;
    let mut req = http().get(&url);
    if let Some(t) = bearer {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let resp = req.send().await.map_err(|e| format!("GET {url}: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status} for {url}: {}", text.chars().take(200).collect::<String>()));
    }
    serde_json::from_str(&text).map_err(|e| format!("bad JSON from {url}: {e}"))
}

async fn catalog_post(base: &str, path: &str, body: Value, bearer: Option<&str>) -> Result<Value, String> {
    let url = catalog_url(base, path)?;
    let mut req = http().post(&url).header("Content-Type", "application/json").body(body.to_string());
    if let Some(t) = bearer {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let resp = req.send().await.map_err(|e| format!("network: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let msg = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v.get("error").and_then(Value::as_str).map(|s| s.to_string()))
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(msg);
    }
    serde_json::from_str(&text).map_err(|e| format!("bad reply: {e}"))
}

/// Names for `ids`, cache-first; misses are fetched `GET <base>/game/<id>` eight at a time (up
/// to CATALOG_FETCH_LIMIT per call — the rest get placeholders and come next refresh).
async fn resolve_catalog(base: &str, ids: &[String], now: u64) -> HashMap<String, CatalogEntry> {
    let mut out: HashMap<String, CatalogEntry> = HashMap::new();
    let mut missing: Vec<String> = Vec::new();
    if let Ok(c) = catalog_cache().lock() {
        for id in ids {
            match c.get(id) {
                Some(e) if e.ok || now.saturating_sub(e.fetched_at) < CATALOG_RETRY_MS => {
                    out.insert(id.clone(), e.clone());
                }
                _ => missing.push(id.clone()),
            }
        }
    }
    missing.truncate(CATALOG_FETCH_LIMIT);
    for chunk in missing.chunks(8) {
        let results = join_all(chunk.iter().map(|id| async move { (id.clone(), catalog_get(base, &format!("/game/{}", enc(id))).await) })).await;
        for (id, res) in results {
            let e = match res {
                Ok(v) => entry_from_json(v.get("game").unwrap_or(&v), now),
                Err(err) => {
                    eprintln!("[games] catalog {id}: {err}");
                    CatalogEntry { fetched_at: now, ..Default::default() }
                }
            };
            if let Ok(mut c) = catalog_cache().lock() {
                c.insert(id.clone(), e.clone());
            }
            out.insert(id, e);
        }
    }
    out
}

/// Percent-encode for a path segment / query value (`:` kept — it is legal in a path and every
/// game id has one).
pub(crate) fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b':' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Poll: notice other people's spins / prefs / settings, and the 30 s write-back fallback
// ---------------------------------------------------------------------------------------------

/// Cheap fingerprint of the container's game-night state in the LOCAL store (the sync loop
/// keeps it current); a change queues a refresh. Same inputs as `load_room_data`'s.
async fn fingerprint(room: &Room, session_id: &str) -> u64 {
    let mut fp: u64 = 0xcbf29ce484222325;
    if let Some(s) = crate::appstate::get_state(room, SETTINGS_TYPE, "").await {
        fnv(&mut fp, &s.get("rev").and_then(Value::as_u64).unwrap_or(0).to_string());
    }
    for t in [PREFS_TYPE, ADJUST_TYPE, SPIN_TYPE] {
        for ev in get_state_full(room, t).await {
            if ev.get("content").map(|c| c.as_object().map(|o| !o.is_empty()).unwrap_or(false)).unwrap_or(false) {
                if t == PREFS_TYPE && ev["state_key"] != ev["sender"] {
                    continue;
                }
                fnv(&mut fp, ev["event_id"].as_str().unwrap_or(""));
            }
        }
    }
    for ev in get_state_full(room, calendar::EVENT_TYPE).await {
        if ev["state_key"] == session_id && ev.get("content").and_then(|c| c.get("title")).and_then(Value::as_str).map(|t| !t.trim().is_empty()).unwrap_or(false) {
            fnv(&mut fp, ev["event_id"].as_str().unwrap_or(""));
        }
    }
    for ev in get_state_full(room, calendar::RSVP_TYPE).await {
        if ev.get("content").and_then(Value::as_object).map(|m| m.contains_key(session_id)).unwrap_or(false) {
            fnv(&mut fp, ev["event_id"].as_str().unwrap_or(""));
        }
    }
    fnv(&mut fp, &model::adhoc_session_id(crate::appstate::now_ms()));
    // 10.5.2 (Astra release review #4): who is an admin decides which adjustments count
    for ev in get_state_full(room, POWER_LEVELS_TYPE).await {
        fnv(&mut fp, ev["event_id"].as_str().unwrap_or(""));
    }
    fp
}

async fn poll_loop(qt: QtThread) {
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(POLL_MS)).await;
        let (cid, session, due, last) = match poll_state().lock() {
            Ok(ps) => (ps.cid.clone(), ps.session_id.clone(), ps.writeback_due, ps.fingerprint),
            Err(_) => continue,
        };
        if cid.is_empty() {
            continue;
        }
        let Some(room) = container_room(&cid) else { continue };
        let now = crate::appstate::now_ms();
        let fp = fingerprint(&room, &session).await;
        let changed = last != 0 && fp != last;
        let writeback_due = due > 0 && now >= due;
        if changed || writeback_due {
            if let Ok(mut ps) = poll_state().lock() {
                ps.fingerprint = fp;
                if writeback_due {
                    ps.writeback_due = 0; // refresh() re-arms it if the calendar is still unset
                }
            }
            let _ = qt.queue(|mut o| o.as_mut().refresh());
        } else if last == 0 {
            if let Ok(mut ps) = poll_state().lock() {
                ps.fingerprint = fp;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Headless proof (10.5.1) — no GUI, against the REAL homeserver, test rooms only:
//   TINHAT_GAMES_TEST='!room:server' ./tinhat     (TINHAT_GAMES_KEEP=1 keeps the spin + my prefs)
//   TINHAT_GAMES_CLEAN='!room:server' ./tinhat    (empties every spin record I wrote there + my prefs)
// ---------------------------------------------------------------------------------------------

fn has_float(v: &Value) -> bool {
    match v {
        Value::Number(n) => n.is_f64(),
        Value::Array(a) => a.iter().any(has_float),
        Value::Object(o) => o.values().any(has_float),
        _ => false,
    }
}

/// The container of `room_id`, refusing any room/space with a member who isn't a test account
/// (a spin or prefs write in a friends' space would show up in their reel).
async fn headless_container(client: &Client, room_id: &str) -> Option<(String, Room)> {
    let Some(cid) = calendar::container_for_room(client, room_id).await else {
        eprintln!("[gamestest] no container for {room_id}");
        return None;
    };
    let room = container_room(&cid)?;
    let members = room.members(matrix_sdk::RoomMemberships::JOIN | matrix_sdk::RoomMemberships::INVITE).await.unwrap_or_default();
    // Test accounts = the signed-in (test) account's name stem — its localpart without trailing digits, so
    // tester1 admits tester2 — on its own server, plus that server's recorder.
    let own = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let Some((own_local, server)) = own.split_once(':') else {
        eprintln!("[gamestest] REFUSING {cid}: no signed-in user id");
        return None;
    };
    let stem = own_local.trim_end_matches(|c: char| c.is_ascii_digit());
    let on_server = format!(":{server}");
    let recorder = crate::server_offer::recorder().unwrap_or_default(); // "" = none announced: matches nobody
    let bad: Vec<String> = members
        .iter()
        .map(|m| m.user_id().to_string())
        .filter(|u| !(stem.len() > 1 && u.starts_with(stem) && u.ends_with(&on_server)) && *u != recorder)
        .collect();
    if members.is_empty() || !bad.is_empty() {
        eprintln!("[gamestest] REFUSING {cid}: members {} non-test {bad:?}", members.len());
        return None;
    }
    eprintln!("[gamestest] room {room_id} → container {cid} ({} test members)", members.len());
    Some((cid, room))
}

/// 10.5.2 credits, headless (`TINHAT_GAMES_TEST=<room>` + `TINHAT_GAMES_CREDITS=<action>`; the web e2e
/// tools/reel-credits-e2e.mjs drives it as a test account's desktop device): the REAL app path —
/// load_room_data (adjustments merged by who is an admin now, `verify`'s credit flag rule) + compute —
/// printed as one `[gamestest] CREDITS {json}` line for the session in `TINHAT_GAMES_SESSION` (else
/// tonight's). Actions, run first (admin only, test rooms only): `read`, `reset` (Reset all credits),
/// `zero:<mxid>`, `adjust:<mxid>:<±n>`, `flag:<spinId>:<0|1>` ("Don't count" / "Count for credit").
pub async fn headless_credits(room_id: &str, action: &str) {
    let Some(client) = crate::session::client() else {
        eprintln!("[gamestest] not signed in");
        return;
    };
    let Some((cid, _room)) = headless_container(&client, room_id).await else {
        eprintln!("[gamestest] VERDICT: FAIL");
        return;
    };
    let session = std::env::var("TINHAT_GAMES_SESSION").unwrap_or_default();
    let base = default_catalog_base();
    let parts: Vec<&str> = action.split(':').collect();
    let res: Result<(), String> = match parts.as_slice() {
        ["read"] | [""] => Ok(()),
        ["reset"] => do_reset_credits(&cid).await,
        ["zero", who @ ..] => do_zero_credit(&cid, &who.join(":")).await,
        ["adjust", rest @ ..] if rest.len() >= 2 => {
            let (who, n) = rest.split_at(rest.len() - 1);
            match n[0].parse::<i64>() {
                Ok(d) => do_adjust(&cid, &base, &session, &[], &who.join(":"), d, "headless").await,
                Err(_) => Err(format!("bad delta {}", n[0])),
            }
        }
        ["flag", id, counts] => do_set_spin_credit(&cid, id, *counts == "1").await,
        _ => Err(format!("unknown action {action}")),
    };
    eprintln!("[gamestest] action {action}: {res:?}");
    for _ in 0..2 {
        let _ = client.sync_once(crate::bridge::sync_settings().timeout(std::time::Duration::from_secs(10))).await;
    }
    let Some(data) = load_room_data(&client, &cid, &base, &session, &[]).await else {
        eprintln!("[gamestest] VERDICT: FAIL (couldn't read the container)");
        return;
    };
    let c = compute(&data, &base, &session, &[], &data.settings, crate::appstate::now_ms());
    let log: Vec<Value> = c.json["log"].as_array().into_iter().flatten().map(|l| json!({ "sessionId": l["sessionId"], "spinId": l["spinId"], "winner": l["winner"], "credit": l["credit"], "voided": l["voided"], "verified": l["verified"] })).collect();
    let out = json!({
        "session": c.session_id,
        "isAdmin": c.is_admin,
        "creditsMode": c.json["creditsMode"],
        "credits": c.json["credits"],
        "spin": { "id": c.json["spin"]["id"], "credit": c.json["spin"]["credit"], "winner": c.json["spin"]["winner"], "verified": c.json["spin"]["verified"] },
        "log": log,
        "action": action,
        "ok": res.is_ok(),
    });
    eprintln!("[gamestest] CREDITS {out}");
    eprintln!("[gamestest] VERDICT: {}", if res.is_ok() { "PASS" } else { "FAIL" });
}

/// Me alone, Flexible, three games, no call, no calendar session — the 10.5.0 dead end (Σw = 0,
/// "Pick a session first.", and a float spin record Synapse refused) — must now spin, land a
/// verified integer record on tonight's ad-hoc session and (as an admin) save Tune settings.
pub async fn headless_verify(room_id: &str) {
    // 10.5.2 credits: `TINHAT_GAMES_CREDITS=<action>` runs the credits hook instead (nothing else is
    // written, and TINHAT_GAMES_KEEP is implied — the e2e cleans up)
    if let Ok(action) = std::env::var("TINHAT_GAMES_CREDITS") {
        headless_credits(room_id, &action).await;
        return;
    }
    let Some(client) = crate::session::client() else {
        eprintln!("[gamestest] not signed in");
        return;
    };
    let Some((cid, room)) = headless_container(&client, room_id).await else {
        eprintln!("[gamestest] VERDICT: FAIL");
        return;
    };
    let sync = || async {
        let _ = client.sync_once(crate::bridge::sync_settings().timeout(std::time::Duration::from_secs(10))).await;
    };
    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let base = default_catalog_base();
    let mut fails = 0u32;
    let mut check = |ok: bool, what: String| {
        eprintln!("[gamestest] {} {what}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            fails += 1;
        }
    };
    let prefs = json!({
        "wouldPlay": ["steam:892970", "steam:108600", "steam:1623730"], "favorites": [], "definitelyNot": [], "flexible": true,
        "names": { "steam:892970": "Valheim", "steam:108600": "Project Zomboid", "steam:1623730": "Palworld" }
    });
    let r = do_save_prefs(&cid, &prefs.to_string()).await;
    check(r.is_ok(), format!("prefs saved: me alone, Flexible, 3 games ({r:?})"));
    sync().await;
    let Some(data) = load_room_data(&client, &cid, &base, "", &[]).await else {
        eprintln!("[gamestest] VERDICT: FAIL (couldn't read the container)");
        return;
    };
    let now = crate::appstate::now_ms();
    let c = compute(&data, &base, "", &[], &data.settings, now);
    check(model::is_adhoc_session(&c.session_id), format!("no calendar session picked → tonight's ad-hoc session {}", c.session_id));
    check(c.roster_actual == vec![me.clone()], format!("open roster outside a call = just me ({:?})", c.roster_actual));
    let reel_len = c.json["reel"].as_array().map(|a| a.len()).unwrap_or(0);
    check(reel_len == 3, format!("one flexible person still fills the reel with even odds ({reel_len} games)"));
    check(c.can_spin || c.standing.is_some(), format!("Spin enabled alone (notice {:?})", c.notice));
    if c.standing.is_some() {
        eprintln!("[gamestest] a spin already stands for {} — run TINHAT_GAMES_CLEAN first", c.session_id);
    }
    let r = do_spin(&cid, &base, "", &[], SpinMode::Initial).await;
    check(r.is_ok(), format!("spin accepted by Synapse (10.5.0: M_BAD_JSON float) → {r:?}"));
    sync().await;
    sync().await;
    let mine: Option<Value> = get_state_full(&room, SPIN_TYPE)
        .await
        .into_iter()
        .filter(|e| e["sender"].as_str() == Some(me.as_str()) && e["content"]["sessionId"].as_str() == Some(c.session_id.as_str()))
        .max_by_key(|e| e["origin_server_ts"].as_u64().unwrap_or(0));
    match &mine {
        Some(ev) => {
            let content = &ev["content"];
            check(!has_float(content), format!("record has no floats: pool {}", content["pool"]));
            let ints = content["pool"].as_array().map(|a| a.iter().all(|g| g["weight"].is_u64() || g["weight"].is_i64())).unwrap_or(false);
            check(ints, "pool weights are integer micro-units".into());
            check(content["seed"].as_str().map(|s| !s.is_empty()).unwrap_or(false) && content["winner"].is_string(), format!("second write landed: seed {} winner {}", content["seed"], content["winner"]));
            check(content["roster"] == json!([me]), format!("roster at spin = me ({})", content["roster"]));
        }
        None => check(false, "no spin record of mine on tonight's session".into()),
    }
    if let Some(d2) = load_room_data(&client, &cid, &base, "", &[]).await {
        let c2 = compute(&d2, &base, "", &[], &d2.settings, crate::appstate::now_ms());
        match &c2.standing {
            Some(st) => {
                check(st.verified && !st.tampered, format!("seed fetched, record verified (verified {} tampered {})", st.verified, st.tampered));
                check(st.computed_winner.is_some() && st.computed_winner == st.winner, format!("winner re-derived from the seed = the written one ({:?})", st.computed_winner));
                check(c2.json["session"]["adhoc"] == json!(true), "snapshot session is the ad-hoc one".into());
            }
            None => check(false, "the spin stands".into()),
        }
        // Tune (admin only): the float-free settings wire form, then restore what was there.
        if c2.is_admin {
            let prev = crate::appstate::get_state(&room, SETTINGS_TYPE, "").await.unwrap_or_else(|| json!({}));
            let r = do_save_settings(&cid, &json!({ "regret.cap": 4 }).to_string()).await;
            check(r.is_ok(), format!("Tune save accepted (10.5.0: every save failed) → {r:?}"));
            sync().await;
            let now_s = crate::appstate::get_state(&room, SETTINGS_TYPE, "").await.unwrap_or_default();
            check(!has_float(&now_s) && now_s["regret.cap"] == json!(4), format!("stored settings float-free, regret.cap {}", now_s["regret.cap"]));
            let r = do_save_settings(&cid, &json!({ "favorite.share": 0.6 }).to_string()).await;
            check(r == Err(model::FLOAT_REFUSAL.to_string()), format!("non-default fraction refused before the server ({r:?})"));
            let restored = send_state_id(&room, SETTINGS_TYPE, "", prev).await;
            check(restored.is_ok(), "settings restored".into());
        } else {
            eprintln!("[gamestest] (not an admin here — Tune save not exercised)");
        }
    }
    // 10.5.2 admin "Reset spins" (the web's, same wire result): re-spin to the limit, reset, and
    // the next spin starts fresh — full re-spins, no vetoes, chained onto the old tail.
    let snap = |c: &Computed| (c.standing.as_ref().map(|s| s.id.clone()), c.respins_left, c.json["spin"]["respinsUsed"].as_u64());
    match load_room_data(&client, &cid, &base, "", &[]).await {
        Some(d) if compute(&d, &base, "", &[], &d.settings, crate::appstate::now_ms()).is_admin => {
            let limit = d.settings.veto_respin_limit;
            let mut last = compute(&d, &base, "", &[], &d.settings, crate::appstate::now_ms());
            for _ in 0..limit {
                if last.standing.is_none() || last.respins_left == 0 {
                    break;
                }
                let r = do_spin(&cid, &base, "", &[], SpinMode::Respin).await;
                check(r.is_ok(), format!("re-spin accepted ({r:?})"));
                sync().await;
                let Some(d) = load_room_data(&client, &cid, &base, "", &[]).await else { break };
                last = compute(&d, &base, "", &[], &d.settings, crate::appstate::now_ms());
            }
            let (tail, left, used) = snap(&last);
            check(tail.is_some() && left == 0 && used == Some(limit as u64), format!("re-spun to the limit: 0 of {limit} re-spins left (standing {tail:?}, used {used:?})"));
            let r = do_reset_spins(&cid, &base, "", &[]).await;
            check(r.is_ok(), format!("Reset spins accepted ({r:?})"));
            sync().await;
            if let Some(d) = load_room_data(&client, &cid, &base, "", &[]).await {
                let c = compute(&d, &base, "", &[], &d.settings, crate::appstate::now_ms());
                let (st, _, used) = snap(&c);
                check(st.is_none() && c.can_spin && used == Some(0), format!("after the reset nothing stands, Spin enabled, 0 re-spins count (standing {st:?}, used {used:?}, canSpin {})", c.can_spin));
                check(c.json["spin"]["voided"] == json!(true) && c.json["spin"]["voidReason"] == json!(model::RESET_REASON), format!("the panel shows tonight voided: {}", c.json["spin"]["note"]));
                let wire: Vec<Value> = get_state_full(&room, SPIN_TYPE).await.into_iter().filter(|e| e["content"]["sessionId"].as_str() == Some(c.session_id.as_str())).collect();
                let all_void = wire.iter().all(|e| e["content"]["voided"].is_object() && e["content"]["voided"]["at"].is_u64());
                let no_respin = wire.iter().all(|e| e["content"]["kind"] != json!("respin"));
                let resets = wire.iter().filter(|e| e["content"]["kind"] == json!(model::RESET_KIND)).count();
                check(all_void && no_respin && resets == limit as usize && wire.iter().all(|e| !has_float(&e["content"])), format!("wire: {} record(s) all voided (integer at), {resets} re-spin(s) re-kinded \"{}\", no floats", wire.len(), model::RESET_KIND));
            }
            let r = do_spin(&cid, &base, "", &[], SpinMode::Initial).await;
            check(r.is_ok(), format!("a fresh spin after the reset ({r:?})"));
            sync().await;
            if let Some(d) = load_room_data(&client, &cid, &base, "", &[]).await {
                let c = compute(&d, &base, "", &[], &d.settings, crate::appstate::now_ms());
                match &c.standing {
                    Some(st) => check(
                        st.kind == "initial" && st.supersedes == tail && st.vetoed.is_empty() && c.respins_left == limit && st.verified,
                        format!("it stands: kind {}, chained onto the old tail {}, {} of {limit} re-spins left, verified {}", st.kind, st.supersedes == tail, c.respins_left, st.verified),
                    ),
                    None => check(false, "the fresh spin stands".into()),
                }
            }
        }
        _ => eprintln!("[gamestest] (not an admin here — Reset spins not exercised)"),
    }
    eprintln!("[gamestest] VERDICT: {}", if fails == 0 { "PASS" } else { "FAIL" });
    if std::env::var_os("TINHAT_GAMES_KEEP").is_none() {
        headless_clean(room_id).await;
    }
}

/// Empties every spin record I wrote in the (test) container and my prefs.
pub async fn headless_clean(room_id: &str) {
    let Some(client) = crate::session::client() else { return };
    let Some((cid, room)) = headless_container(&client, room_id).await else { return };
    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let mut n = 0;
    for ev in get_state_full(&room, SPIN_TYPE).await {
        let nonempty = ev["content"].as_object().map(|o| !o.is_empty()).unwrap_or(false);
        if nonempty && ev["sender"].as_str() == Some(me.as_str()) {
            if let Some(sk) = ev["state_key"].as_str() {
                if send_state_id(&room, SPIN_TYPE, sk, json!({})).await.is_ok() {
                    n += 1;
                }
            }
        }
    }
    let p = send_state_id(&room, PREFS_TYPE, &me, json!({})).await;
    eprintln!("[gamestest] cleaned {cid}: {n} spin record(s) emptied, prefs cleared {:?}", p.is_ok());
}

#[cfg(test)]
mod tests {
    use super::*;

    const KAI: &str = "@kai:example.com";
    const SAM: &str = "@sam:example.com";
    const JO: &str = "@jo:example.com";

    /// 12.0.1: nothing goes out while the base is still unknown ("/games" before the first sign-in) — a plain
    /// error instead of reqwest's "relative URL without a base".
    #[test]
    fn catalog_url_needs_a_server() {
        assert_eq!(
            catalog_url("https://example.com/api/games", "/steam/library").unwrap(),
            "https://example.com/api/games/steam/library"
        );
        assert!(catalog_url("/games", "/steam/library").is_err());
        assert!(catalog_url("", "/game/steam%3A1").is_err());
    }

    /// 10.5.2 admin "Reset spins": every record the reset rewrites still verifies against its seed
    /// event with the desktop's own `verify` (pool / roster / session / winner untouched; the spinner
    /// stays the seed's sender, not the admin who rewrote it) — the web test's `verify` loop.
    #[test]
    fn reset_rewrites_still_verify_against_their_seeds() {
        let pool: Vec<(String, f64)> = vec![("steam:892970".into(), 1_250_000.0), ("steam:108600".into(), 1_000_000.0), ("steam:1623730".into(), 1_500_000.0)];
        let mk = |id: &str, ts: u64, kind: &str, sup: Option<&str>| {
            let mut r = SpinRecord {
                id: id.into(),
                event_id: format!("$ev{ts}"),
                ts,
                sender: KAI.into(),
                session_id: "s1".into(),
                kind: kind.into(),
                supersedes: sup.map(String::from),
                roster: vec![KAI.into(), SAM.into()],
                pool: pool.clone(),
                ..Default::default()
            };
            let first = r.to_json(); // write 1: no winner, no seed — its event id IS the seed
            r.seed = r.event_id.clone();
            r.winner = model::derive_winner(&r.event_id, &r.pool);
            (r, first)
        };
        let night = [mk("sp-a", 5000, "initial", None), mk("sp-l", 5200, "initial", None), mk("sp-b", 6000, "respin", Some("sp-a")), mk("sp-c", 7000, "respin", Some("sp-b"))];
        let wire0: Vec<model::RawSpin> = night.iter().map(|(r, _)| model::RawSpin { id: r.id.clone(), event_id: format!("$w2-{}", r.ts), content: r.to_json() }).collect();
        let parsed: Vec<SpinRecord> = night.iter().map(|(r, _)| r.clone()).collect();
        let chain = model::resolve_session(&parsed).chain;
        assert_eq!(chain, vec!["sp-a".to_string(), "sp-b".into(), "sp-c".into()]);
        let writes = model::reset_writes(&chain, &wire0, JO, 99);
        assert_eq!(writes.len(), 5);
        let mut last: HashMap<String, Value> = HashMap::new();
        for (id, c) in writes {
            last.insert(id, c);
        }
        for (orig, first) in &night {
            let content = &last[&orig.id];
            // as a reader sees it after the reset: the admin's rewrite, a new event id + clock
            let mut rec = SpinRecord::from_json(&orig.id, "$rewrite", 90_000, JO, content);
            let seed_event = json!({ "origin_server_ts": orig.ts, "sender": KAI, "content": first });
            verify(&mut rec, Some(&seed_event), true);
            assert!(rec.verified && !rec.tampered, "{} verifies after the reset", orig.id);
            assert_eq!(rec.computed_winner, orig.winner, "{} derives the same winner", orig.id);
            assert_eq!((rec.ts, rec.sender.as_str()), (orig.ts, KAI), "{}: the seed's clock + spinner, not the admin's rewrite", orig.id);
            assert_eq!(rec.voided.as_ref().map(|v| v.reason.as_str()), Some(model::RESET_REASON));
        }
        // control: verify does catch a rewrite that touched the pool
        let mut bad = last["sp-b"].clone();
        bad["pool"][0]["weight"] = json!(9_000_000);
        let mut rec = SpinRecord::from_json("sp-b", "$rewrite", 90_000, JO, &bad);
        verify(&mut rec, Some(&json!({ "origin_server_ts": 6000, "sender": KAI, "content": night[2].1 })), true);
        assert!(!rec.verified && rec.tampered);
    }

    /// 10.5.2: a member's rewrite of a spin record can't change its credit flag (the web's games.test.ts
    /// twin); the flag never touches what verification compares, and the spinner stays the seed's sender.
    /// (An admin's "Don't count for credit" is separate admin-only state — model::credit_overrides.)
    #[test]
    fn credit_flag_rewrites_verify_and_only_an_admin_may_change_the_flag() {
        let first = json!({ "v": 1, "sessionId": "s", "kind": "initial", "roster": [KAI, SAM], "pool": [{ "id": "steam:892970", "weight": 1500000 }, { "id": "steam:108600", "weight": 500000 }] });
        let w = model::derive_winner("$seed1", &[("steam:892970".to_string(), 1_500_000.0), ("steam:108600".to_string(), 500_000.0)]);
        let seed_ev = json!({ "origin_server_ts": 9, "sender": KAI, "content": first });
        let mut uncounted = first.clone();
        uncounted["seed"] = json!("$seed1");
        uncounted["winner"] = json!(w);
        uncounted["credit"] = json!(false);
        for (writer_admin, want) in [(false, true), (true, false)] {
            let mut r = SpinRecord::from_json("sp-1", "$third", 20, if writer_admin { JO } else { SAM }, &uncounted);
            verify(&mut r, Some(&seed_ev), writer_admin);
            assert_eq!(r.credit, want, "rewritten by {}", if writer_admin { "an admin" } else { "a member" });
            assert!(r.verified && !r.tampered, "the flag doesn't touch what verification compares");
            assert_eq!(r.sender, KAI, "the spinner stays the seed's sender");
        }
        // a free spin (write 1 carries credit:false): a member's rewrite without the key can't make it count
        let mut free_first = first.clone();
        free_first["credit"] = json!(false);
        let mut back = first.clone();
        back["seed"] = json!("$seed1");
        back["winner"] = json!(w);
        let mut r = SpinRecord::from_json("sp-1", "$x", 20, SAM, &back);
        verify(&mut r, Some(&json!({ "origin_server_ts": 9, "sender": KAI, "content": free_first })), false);
        assert!(!r.credit);
        // seed not fetched: the copy we have
        let mut r = SpinRecord::from_json("sp-1", "$x", 20, SAM, &uncounted);
        verify(&mut r, None, false);
        assert!(!r.credit && !r.verified && !r.tampered);
    }

    #[test]
    fn landing_guard_uses_javascript_truthiness() {
        assert!(!truthy(None));
        for v in [json!(null), json!(false), json!(0), json!(""), json!(0.0)] {
            assert!(!truthy(Some(&v)), "{v} is falsy");
        }
        for v in [json!({}), json!([]), json!(true), json!(1), json!("x"), json!({ "by": JO })] {
            assert!(truthy(Some(&v)), "{v} is truthy");
        }
    }
}
