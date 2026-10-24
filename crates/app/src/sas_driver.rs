//! sas_driver.rs — the Qt-free core of interactive (emoji/SAS) device verification.
//!
//! ONE driver for the app and the TINHAT_SASTEST harness: bridge.rs reports through a Qt sink
//! (qproperties on LoginController), sas_test.rs through a logging sink. The harness therefore
//! exercises the shipped logic; the old sas_test.rs had a hand copy of drive_verification that
//! had drifted (no ACTIVE_SAS, no guard, auto-confirm, and full Debug dumps of SDK state).
//!
//! On top of the 10.6.0 driver (bridge.rs drive_verification) this carries the 2026-09-25 fix for
//! "on an iPhone the screen just goes black and I cannot verify anything" (Element Classic as the
//! other side). Shared spec with Tin Hat Web (src/core/matrix/crypto.ts + sasPolicy.ts):
//!  S1/S2 the REQUESTER starts on Ready only for a SAS-only peer; a peer that also offers QR gets
//!        to pick — we wait, say how, and offer "Start emoji comparison" (sasCanStart).
//!  S3    one start guard for the auto AND the manual start: the request must still be Ready, no
//!        SAS may exist, and we must not have started one for this request. matrix-sdk-crypto's
//!        start_sas on a Transitioned request inserts a second SAS and the cache then cancels
//!        BOTH (cache.rs insert) — the exact start + 2 cancels seen from Classic.
//!  S4    we advertise exactly [m.sas.v1] (sas_policy::our_methods) in request and ready.
//!  S5    a replaced SAS (glare lost after the keys, or the other side's second start) ends its
//!        watcher task, the new SAS's emoji replace the old ones, and "They match" confirms only
//!        the SAS whose emoji are on screen.
//!  S6    cancel notes by wire code (sas_policy::cancel_note).
//!  S7    the verifier's done note says what it did (vouched for the OTHER device).
//!  S8    incoming requests are driven from the moment they arrive (a requester's cancel before
//!        Accept shows as cancelled instead of a stale Accept).
//!
//! Flow incarnation: every flow (and every outgoing attempt, from the click on) has a number
//! `inc`. The user's cancel, a new attempt or a new incoming request moves it on SYNCHRONOUSLY,
//! and every UI write carries the `inc` it belongs to — sinks drop writes of a flow that is no
//! longer live when they are APPLIED, and every command (accept, start, confirm) names the flow
//! the user saw. So a late write or a delayed task of an ended flow can never touch a newer one
//! (Astra review 2026-09-25).
//!
//! Logging: variant names and method names only — never `{state:?}` (the Debug dump serialized
//! multi-KB SDK internals incl. device keys and SAS codes into a log that logship SHIPS).

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex, MutexGuard,
};
use std::time::Duration;

use futures_util::StreamExt;
use matrix_sdk::encryption::verification::{
    Emoji, SasState, SasVerification, Verification, VerificationRequest, VerificationRequestState,
};

use crate::sas_policy::{self, SasStart};

/// Where the driver reports. Calls come from tokio tasks and must not block. Every call names
/// the flow incarnation it belongs to; a sink must drop it if `is_live(inc)` is false by the time
/// the write is APPLIED (the Qt sink checks inside the queued closure).
pub trait SasUi: Send + Sync + 'static {
    /// An incoming request just arrived (S8): sasIncoming=true, no emoji, no buttons but Accept,
    /// sasState "waiting" + NOTE_INCOMING_ARRIVED.
    fn arrived(&self, inc: u64);
    /// sasState + sasNote together.
    fn set_state(&self, inc: u64, state: &str, note: &str);
    /// The other device's name (for "Confirm these emoji appear … on <peer>").
    fn set_peer(&self, inc: u64, peer: &str);
    /// S2: show / hide "Start emoji comparison".
    fn set_can_start(&self, inc: u64, can: bool);
    /// KeysExchanged: the seven emoji as the dialog's JSON, sasState "emoji", and the note.
    fn show_emoji(&self, inc: u64, emoji_json: &str, note: &str);
    /// The comparison finished on this side: sasState "done" with `note`. The app re-checks its
    /// cross-signing state and refreshes rooms here (that part applies even if a newer flow began).
    fn sas_done(&self, inc: u64, note: &str);
}

/// Which requester rules to follow. `Shipped1060` = the 10.6.0 behaviour (start the moment Ready
/// arrives, whatever the peer offers, no guard, no settle; SDK-default methods) and exists ONLY so
/// the harness can reproduce the field failure next to the fix. The app always uses `Current`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // Shipped1060 is built by the Linux-only harness
pub enum Rules {
    Current,
    Shipped1060,
}

struct Active {
    /// The flow incarnation (see the module doc). Moves on at every begin / reserve / user end.
    inc: u64,
    req: Option<VerificationRequest>,
    incoming: bool,
    rules: Rules,
    ui: Option<Arc<dyn SasUi>>,
    sas: Option<SasVerification>,
    /// Bumped for every SAS adopted from Transitioned; a watcher only acts while it is current.
    gen: u64,
    /// S3: we already started a SAS for this request.
    started: bool,
    done_shown: bool,
    driver: Option<tokio::task::JoinHandle<()>>,
    watcher: Option<tokio::task::JoinHandle<()>>,
}

static ACTIVE: Mutex<Active> = Mutex::new(Active {
    inc: 0,
    req: None,
    incoming: false,
    rules: Rules::Current,
    ui: None,
    sas: None,
    gen: 0,
    started: false,
    done_shown: false,
    driver: None,
    watcher: None,
});

/// S3: the auto start waits this long for the rest of the sync batch before its guard re-check.
const AUTO_START_SETTLE: Duration = Duration::from_millis(sas_policy::AUTO_START_SETTLE_MS);

static LIVE_DRIVERS: AtomicUsize = AtomicUsize::new(0);
static LIVE_WATCHERS: AtomicUsize = AtomicUsize::new(0);

/// Counts a live task; the Drop runs on a normal end AND on abort.
struct Live(&'static AtomicUsize);
impl Live {
    fn new(c: &'static AtomicUsize) -> Self {
        c.fetch_add(1, Ordering::SeqCst);
        Live(c)
    }
}
impl Drop for Live {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// (request drivers, SAS watchers) still alive — the harness asserts (0, 0) once a flow is over.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn live_tasks() -> (usize, usize) {
    (LIVE_DRIVERS.load(Ordering::SeqCst), LIVE_WATCHERS.load(Ordering::SeqCst))
}

fn lock() -> MutexGuard<'static, Active> {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Is `inc` still the flow on screen? Sinks call this when a write is applied.
pub fn is_live(inc: u64) -> bool {
    lock().inc == inc
}

/// Is flow `inc` still live AND not finished? Progress writes (emoji, buttons, "waiting") apply
/// only then; terminal writes (cancelled / done) only need `is_live`.
pub fn is_open(inc: u64) -> bool {
    let a = lock();
    a.inc == inc && a.req.is_some()
}

/// The newest incarnation (the Qt side tracks which one it is SHOWING and sends that instead).
pub fn current_inc() -> u64 {
    lock().inc
}

/// The dialog's emoji JSON: `[{"e":"🐶","name":"Dog"}, …]`.
pub fn emoji_json(emojis: &[Emoji]) -> String {
    let items: Vec<String> = emojis
        .iter()
        .map(|e| {
            format!(
                r#"{{"e":{},"name":{}}}"#,
                serde_json::to_string(e.symbol).unwrap_or_else(|_| "\"\"".into()),
                serde_json::to_string(e.description).unwrap_or_else(|_| "\"\"".into())
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

fn state_name(s: &VerificationRequestState) -> &'static str {
    match s {
        VerificationRequestState::Created { .. } => "Created",
        VerificationRequestState::Requested { .. } => "Requested",
        VerificationRequestState::Ready { .. } => "Ready",
        VerificationRequestState::Transitioned { .. } => "Transitioned",
        VerificationRequestState::Done => "Done",
        VerificationRequestState::Cancelled(_) => "Cancelled",
    }
}

fn sas_state_name(s: &SasState) -> &'static str {
    match s {
        SasState::Created { .. } => "Created",
        SasState::Started { .. } => "Started",
        SasState::Accepted { .. } => "Accepted",
        SasState::KeysExchanged { .. } => "KeysExchanged",
        SasState::Confirmed => "Confirmed",
        SasState::Done { .. } => "Done",
        SasState::Cancelled(_) => "Cancelled",
    }
}

/// What an ended flow still holds on the wire (cancel it with `cancel_on_wire`).
pub struct Ended {
    req: Option<VerificationRequest>,
    sas: Option<SasVerification>,
}

impl Ended {
    /// "They don't match" (`mismatch` — m.mismatched_sas, as the web sends) or cancel / dismiss
    /// (m.user). Cancelling is the SAFE outcome — never treat a dismissal as success.
    pub async fn cancel_on_wire(self, mismatch: bool) {
        if let Some(sas) = self.sas {
            let _ = if mismatch { sas.mismatch().await } else { sas.cancel().await };
        }
        if let Some(req) = self.req {
            if !req.is_done() && !req.is_cancelled() {
                let _ = req.cancel().await;
            }
        }
    }
}

/// Move to a new incarnation and forget the current flow (its tasks are aborted); returns the new
/// incarnation and what the old flow still holds on the wire.
fn bump(a: &mut Active) -> (u64, Ended) {
    if let Some(h) = a.driver.take() {
        h.abort();
    }
    if let Some(h) = a.watcher.take() {
        h.abort();
    }
    let ended = Ended { req: a.req.take(), sas: a.sas.take() };
    a.inc += 1;
    a.ui = None;
    a.gen = 0;
    a.started = false;
    a.done_shown = false;
    (a.inc, ended)
}

/// The user cancelled / dismissed / said "they don't match": end the flow NOW (call it on the
/// thread that shows the dialog, before anything else is written), then `cancel_on_wire`.
pub fn end_by_user() -> Ended {
    bump(&mut lock()).1
}

/// The user asked to verify this device: a new outgoing attempt begins. Returns its incarnation
/// (hand it to `begin_outgoing` once the request exists) and the previous flow's leftovers.
pub fn reserve() -> (u64, Ended) {
    bump(&mut lock())
}

/// Drive the outgoing request of attempt `inc`. False = the attempt was cancelled or superseded
/// while the request was being created — the caller cancels the request on the wire.
/// Must be called inside the tokio runtime.
pub fn begin_outgoing(req: VerificationRequest, inc: u64, ui: Arc<dyn SasUi>, rules: Rules) -> bool {
    {
        let mut a = lock();
        if a.inc != inc {
            return false;
        }
        a.req = Some(req.clone());
        a.incoming = false;
        a.rules = rules;
        a.ui = Some(ui.clone());
    }
    ui.set_can_start(inc, false);
    spawn_driver(req, inc);
    true
}

/// S8: an incoming request becomes THE flow the moment it arrives (not after Accept), so a
/// requester's cancel shows before Accept. Returns its incarnation. Inside the tokio runtime.
pub fn begin_incoming(req: VerificationRequest, ui: Arc<dyn SasUi>, rules: Rules) -> u64 {
    let inc = {
        let mut a = lock();
        let (inc, _previous) = bump(&mut a);
        a.req = Some(req.clone());
        a.incoming = true;
        a.rules = rules;
        a.ui = Some(ui.clone());
        inc
    };
    ui.arrived(inc);
    spawn_driver(req, inc);
    inc
}

fn spawn_driver(req: VerificationRequest, inc: u64) {
    let h = tokio::spawn(drive(req, inc));
    let mut a = lock();
    if a.inc == inc && a.req.is_some() {
        a.driver = Some(h);
    } else {
        h.abort();
    }
}

/// The reporting context of `inc`, if it is still the active flow.
fn current(inc: u64) -> Option<(Arc<dyn SasUi>, bool, Rules)> {
    let a = lock();
    if a.inc == inc && a.req.is_some() {
        a.ui.clone().map(|u| (u, a.incoming, a.rules))
    } else {
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Caller {
    /// The request driver task itself (not aborted by its own cleanup).
    Driver,
    /// The watcher task of SAS generation N (checked; not aborted by its own cleanup).
    Watcher(u64),
    /// Work on SAS generation N outside its watcher (a detached accept, a confirm): checked
    /// under the cleanup lock, so an obsolete comparison's error never ends its replacement.
    Sas(u64),
    Other,
}

/// Terminal: forget flow `inc` if it (and, for a watcher, its generation) is still the active
/// one — checked and cleared under ONE lock, so an obsolete task can never tear down its
/// replacement. Aborts the other task; the caller's own task ends by itself. Returns what the
/// flow held on the wire, or None if it was not current.
fn finish(inc: u64, who: Caller) -> Option<Ended> {
    let mut a = lock();
    if a.inc != inc || a.req.is_none() {
        return None;
    }
    if let Caller::Watcher(gen) | Caller::Sas(gen) = who {
        if a.gen != gen {
            return None;
        }
    }
    let ended = Ended { req: a.req.take(), sas: a.sas.take() };
    let (d, w) = (a.driver.take(), a.watcher.take());
    drop(a);
    if who != Caller::Driver {
        if let Some(h) = d {
            h.abort();
        }
    }
    if !matches!(who, Caller::Watcher(_)) {
        if let Some(h) = w {
            h.abort();
        }
    }
    Some(ended)
}

/// A local failure the dialog shows as cancelled: end the flow and cancel it on the wire too,
/// so the other device isn't left waiting for ten minutes.
fn fail(inc: u64, who: Caller, note: &str) {
    let ui = current(inc).map(|c| c.0);
    let Some(ended) = finish(inc, who) else { return };
    eprintln!("[sas] ending the flow: {note}");
    if let Some(ui) = ui {
        ui.set_can_start(inc, false);
        ui.set_state(inc, "cancelled", note);
    }
    tokio::spawn(ended.cancel_on_wire(false));
}

async fn drive(req: VerificationRequest, inc: u64) {
    let _live = Live::new(&LIVE_DRIVERS);
    let mut changes = req.changes();
    eprintln!("[sas] driver watching flow_id={} we_started={} (flow #{inc})", req.flow_id(), req.we_started());
    // Seed with the CURRENT state: changes() does NOT replay it, so a transition that landed
    // between the caller's last await and this subscribe would otherwise be lost forever — the
    // accepting side then never sees Transitioned and goes deaf (found via the sas_test.rs start-race harness).
    let mut pending = Some(req.state());
    loop {
        let state = match pending.take() {
            Some(s) => s,
            None => match changes.next().await {
                Some(s) => s,
                None => break,
            },
        };
        let Some((ui, incoming, rules)) = current(inc) else {
            eprintln!("[sas] request {} of an ended flow (#{inc}) — its driver ends", state_name(&state));
            break;
        };
        eprintln!("[sas] request state -> {}", state_name(&state));
        match state {
            VerificationRequestState::Requested { .. } => {
                if !req.we_started() {
                    ui.set_state(inc, "waiting", sas_policy::NOTE_INCOMING_REQUESTED);
                }
            }
            VerificationRequestState::Ready { their_methods, other_device_data, .. } => {
                let peer = other_device_data
                    .display_name()
                    .map(str::to_owned)
                    .unwrap_or_else(|| other_device_data.device_id().to_string());
                ui.set_peer(inc, &peer);
                let names = sas_policy::method_names(&their_methods);
                // Only the side that sent the request ever starts the SAS (S1); the verifier
                // waits for the requester's start and accepts it by the SAS's own state.
                if !req.we_started() {
                    eprintln!("[sas] ready (we are the verifier): their methods [{names}]");
                    continue;
                }
                let policy = sas_policy::sas_start_policy(&their_methods);
                eprintln!("[sas] ready: their methods [{names}] -> {}", policy.as_str());
                if rules == Rules::Shipped1060 {
                    start_unguarded(&req, inc).await;
                    continue;
                }
                match policy {
                    SasStart::Start => {
                        // Let the rest of this sync batch land first: after a phone resumes (or
                        // any catch-up sync) the peer's ready AND its own start arrive together,
                        // and the SDK emits Ready while the start is still being processed.
                        // Starting inside that window makes the SDK cancel both comparisons
                        // (TINHAT_SASTEST_SUSPEND reproduces it); after the settle the guard sees
                        // Transitioned and we accept theirs instead. (Narrows the window; the
                        // SDK offers no atomic "start only if still Ready".)
                        tokio::time::sleep(AUTO_START_SETTLE).await;
                        start_guarded(&req, inc, "auto").await
                    }
                    SasStart::Wait => {
                        // Their user picks the method (Element Classic / Element Web). Starting
                        // now is what left Classic on its dark "Scan QR code" screen. Wait for
                        // their start (accepted by state below) — or our user's button.
                        ui.set_state(inc, "waiting", &sas_policy::wait_note(&peer));
                        ui.set_can_start(inc, true);
                    }
                    SasStart::NoSas => fail(inc, Caller::Driver, sas_policy::NOTE_NO_SAS),
                }
            }
            VerificationRequestState::Transitioned { verification, .. } => match verification {
                Verification::SasV1(sas) => adopt(inc, sas),
                #[allow(unreachable_patterns)]
                _ => eprintln!("[sas] transitioned to a non-emoji verification — ignored"),
            },
            VerificationRequestState::Done => {
                let show = {
                    let mut a = lock();
                    let show = a.inc == inc && !a.done_shown;
                    if show {
                        a.done_shown = true;
                    }
                    show
                };
                // The SAS watcher normally reports first (with the fuller note); this is the
                // fallback when the request's done lands first.
                if show {
                    ui.set_can_start(inc, false);
                    ui.set_state(
                        inc,
                        "done",
                        if incoming { sas_policy::NOTE_DONE_VERIFIER } else { sas_policy::NOTE_DONE_REQUESTER_REQUEST },
                    );
                }
                break;
            }
            VerificationRequestState::Cancelled(info) => {
                let code = info.cancel_code().as_str().to_owned();
                eprintln!("[sas] request cancelled: code={code} by_us={}", info.cancelled_by_us());
                if finish(inc, Caller::Driver).is_some() {
                    ui.set_can_start(inc, false);
                    ui.set_state(inc, "cancelled", &sas_policy::cancel_note(Some(&code)));
                }
                break;
            }
            VerificationRequestState::Created { .. } => {}
        }
    }
}

/// Transitioned: follow this SAS. A second Transitioned for the same request is a replacement
/// (S5): the previous watcher ends here, so no orphan keeps reporting a dead comparison.
fn adopt(inc: u64, sas: SasVerification) {
    let (ui, incoming, gen) = {
        let mut a = lock();
        if a.inc != inc || a.req.is_none() {
            return;
        }
        let replaced = a.sas.is_some();
        if let Some(h) = a.watcher.take() {
            h.abort();
        }
        if replaced {
            eprintln!("[sas] the comparison was replaced (start race / their second start) — following the new one");
        }
        a.gen += 1;
        a.sas = Some(sas.clone());
        (a.ui.clone(), a.incoming, a.gen)
    };
    let Some(ui) = ui else { return };
    ui.set_can_start(inc, false);
    let h = tokio::spawn(watch_sas(inc, gen, sas, ui, incoming));
    let mut a = lock();
    if a.inc == inc && a.req.is_some() && a.gen == gen {
        a.watcher = Some(h);
    } else {
        h.abort();
    }
}

fn current_sas(inc: u64, gen: u64) -> bool {
    let a = lock();
    a.inc == inc && a.req.is_some() && a.gen == gen
}

async fn watch_sas(inc: u64, gen: u64, sas: SasVerification, ui: Arc<dyn SasUi>, incoming: bool) {
    let _live = Live::new(&LIVE_WATCHERS);
    let mut changes = sas.changes();
    // Same no-replay hazard as the request stream: seed with the current SAS state or a fast
    // peer's Accepted/KeysExchanged that beat this subscribe is lost (and the emoji never show).
    let mut pending = Some(sas.state());
    loop {
        let st = match pending.take() {
            Some(s) => s,
            None => match changes.next().await {
                Some(s) => s,
                None => break,
            },
        };
        if !current_sas(inc, gen) {
            eprintln!("[sas] sas state {} for a replaced comparison — its watcher ends", sas_state_name(&st));
            break;
        }
        eprintln!("[sas] sas state -> {}", sas_state_name(&st));
        match st {
            SasState::Started { .. } => {
                // Accept when THIS SAS was started by the other side — judged by the SAS's own
                // state, NOT `!sas.we_started()`: matrix-sdk stamps the REQUEST initiator flag onto
                // every Sas it builds from an incoming start, so after a start race the winning
                // replacement still reads we_started=true on the losing requester.
                // Started = theirs, needs our m.key.verification.accept; Created = ours.
                // Detached from this watcher: accept() flips the local state BEFORE it sends, so
                // an abort in between (a re-adoption) would leave both sides waiting forever.
                eprintln!("[sas] their start is in effect — accepting their SAS");
                let s2 = sas.clone();
                tokio::spawn(async move {
                    if let Err(e) = s2.accept().await {
                        // Generation-checked inside fail(): an obsolete comparison's failure
                        // never ends its replacement (a stale one is only logged).
                        eprintln!("[sas] accepting their start failed (comparison #{gen}): {e}");
                        fail(inc, Caller::Sas(gen), &format!("Could not accept their start: {e}"));
                    }
                });
            }
            SasState::KeysExchanged { emojis, .. } => match emojis {
                Some(e) => ui.show_emoji(inc, &emoji_json(&e.emojis), sas_policy::NOTE_EMOJI),
                // The other side negotiated numbers only; this dialog compares emoji.
                None => fail(
                    inc,
                    Caller::Watcher(gen),
                    &sas_policy::cancel_note(Some("m.unknown_method")),
                ),
            },
            SasState::Done { .. } => {
                {
                    let mut a = lock();
                    if a.inc == inc && a.gen == gen {
                        a.done_shown = true;
                    }
                }
                if finish(inc, Caller::Watcher(gen)).is_some() {
                    ui.set_can_start(inc, false);
                    ui.sas_done(inc, if incoming { sas_policy::NOTE_DONE_VERIFIER } else { sas_policy::NOTE_DONE_REQUESTER });
                }
                break;
            }
            SasState::Cancelled(info) => {
                let code = info.cancel_code().as_str().to_owned();
                eprintln!("[sas] comparison cancelled: code={code} by_us={}", info.cancelled_by_us());
                // The dialog now says cancelled (with Try again): nothing may keep running behind
                // it. The SDK can cancel a SAS without its request (its cache cancels BOTH
                // comparisons when a second one is inserted), which would otherwise leave the
                // request driver waiting for up to ten minutes.
                if let Some(ended) = finish(inc, Caller::Watcher(gen)) {
                    ui.set_can_start(inc, false);
                    ui.set_state(inc, "cancelled", &sas_policy::cancel_note(Some(&code)));
                    // The REQUEST can still be open here (the SDK cache cancels comparisons, not their request;
                    // a comparison timeout or a bad MAC likewise). matrix-sdk-crypto cancels every NEW
                    // verification request of this account at birth while an older one is open
                    // (VerificationMachine::insert_request) — "Try again" would die at once for up to ten
                    // minutes, and its request would even go out with the SDK's default methods. End it.
                    if let Some(req) = ended.req {
                        park_leftover(req);
                    }
                }
                break;
            }
            _ => {}
        }
    }
}

/// A request whose comparison ended cancelled while the request itself stayed open (watch_sas). It is
/// cancelled after LEFTOVER_DELAY, so the SDK's own, more specific cancel of the comparison (e.g.
/// m.mismatched_sas, sent with the next sync) reaches the other device before our request's plain
/// m.user; a new attempt ends it at once first (end_leftover), so its own request is not cancelled at
/// birth.
static LEFTOVER: Mutex<Option<VerificationRequest>> = Mutex::new(None);
const LEFTOVER_DELAY: Duration = Duration::from_millis(1500);

fn park_leftover(req: VerificationRequest) {
    if req.is_done() || req.is_cancelled() {
        return;
    }
    let flow = req.flow_id().to_owned();
    // A different one still parked (two comparisons ended within LEFTOVER_DELAY): end it now, don't drop it.
    let prev = LEFTOVER.lock().unwrap_or_else(|e| e.into_inner()).replace(req);
    if let Some(p) = prev.filter(|p| p.flow_id() != flow) {
        tokio::spawn(end_open(p));
    }
    tokio::spawn(async move {
        tokio::time::sleep(LEFTOVER_DELAY).await;
        let mine = {
            let mut g = LEFTOVER.lock().unwrap_or_else(|e| e.into_inner());
            if g.as_ref().is_some_and(|r| r.flow_id() == flow) { g.take() } else { None }
        };
        if let Some(r) = mine {
            end_open(r).await;
        }
    });
}

async fn end_open(r: VerificationRequest) {
    if !r.is_done() && !r.is_cancelled() {
        eprintln!("[sas] ending the open request of a cancelled comparison (flow {})", r.flow_id());
        let _ = r.cancel().await;
    }
}

/// A new attempt is about to ask: end a parked open request first (see LEFTOVER). Inside the runtime.
pub async fn end_leftover() {
    let r = LEFTOVER.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(r) = r {
        end_open(r).await;
    }
}

/// S3 — the ONLY way the current rules start a SAS (auto on Ready, or the button).
async fn start_guarded(req: &VerificationRequest, inc: u64, why: &str) {
    {
        let mut a = lock();
        let skip = if a.inc != inc || a.req.is_none() {
            Some("it is not the active request".to_string())
        } else if !req.we_started() {
            Some("we are the verifier, the other device starts".to_string())
        } else if a.started {
            Some("we already started one for this request".to_string())
        } else if a.sas.is_some() {
            Some("a comparison already exists".to_string())
        } else {
            match req.state() {
                VerificationRequestState::Ready { .. } => None,
                s => Some(format!("the request is {} now, not Ready", state_name(&s))),
            }
        };
        if let Some(why_not) = skip {
            eprintln!("[sas] start skipped ({why}): {why_not}");
            return;
        }
        a.started = true;
    }
    if let Some((ui, _, _)) = current(inc) {
        ui.set_can_start(inc, false);
    }
    eprintln!("[sas] starting the emoji comparison ({why}) on a Ready request");
    match req.start_sas().await {
        // The driver's Transitioned arm adopts it.
        Ok(Some(_)) => {}
        Ok(None) => {
            if matches!(req.state(), VerificationRequestState::Ready { .. }) {
                fail(inc, Caller::Other, sas_policy::NOTE_NO_SAS);
            } else {
                eprintln!("[sas] start_sas -> None (the request moved on meanwhile)");
            }
        }
        Err(e) => fail(inc, Caller::Other, &format!("Could not start: {e}")),
    }
}

/// 10.6.0's start (harness reproduction only): at once on Ready, no re-check.
async fn start_unguarded(req: &VerificationRequest, inc: u64) {
    eprintln!("[sas] starting the emoji comparison (shipped-10.6.0 rule: at once on Ready, no guard)");
    lock().started = true;
    match req.start_sas().await {
        Ok(Some(_)) => {}
        Ok(None) => fail(inc, Caller::Other, sas_policy::NOTE_NO_SAS),
        Err(e) => fail(inc, Caller::Other, &format!("Could not start: {e}")),
    }
}

/// S2 — "Start emoji comparison" pressed while flow `inc` was on screen (through the S3 guard).
pub async fn start_now(inc: u64) {
    let req = {
        let a = lock();
        if a.inc != inc || a.incoming {
            None
        } else {
            a.req.clone()
        }
    };
    let Some(req) = req else {
        eprintln!("[sas] manual start: flow #{inc} is not an outgoing request in progress");
        return;
    };
    start_guarded(&req, inc, "manual").await;
}

/// The verifier accepts incoming flow `inc` (S4: advertising [m.sas.v1] only). The driver has
/// been following the request since it arrived (S8), so nothing else starts here.
pub async fn accept(inc: u64) -> Result<(), String> {
    let (req, rules) = {
        let a = lock();
        if a.inc != inc {
            eprintln!("[sas] accept: flow #{inc} has ended");
            return Ok(());
        }
        (a.req.clone(), a.rules)
    };
    let Some(req) = req else {
        eprintln!("[sas] accept: no request in progress (cancelled meanwhile?)");
        return Ok(());
    };
    let r = match rules {
        Rules::Current => req.accept_with_methods(sas_policy::our_methods()).await,
        Rules::Shipped1060 => req.accept().await,
    };
    r.map_err(|e| e.to_string())
}

pub enum Confirm {
    Sent,
    /// Sending the confirmation failed — hand the generation to `fail_confirm`.
    Failed(u64, String),
    /// Flow `inc` ended, or has no comparison any more (its terminal state is on its way).
    NoSas,
    /// The emoji on screen are not the active comparison's: nothing was confirmed. Carries the
    /// active comparison's emoji JSON when it already has some (show those instead).
    Stale(Option<String>),
}

/// "They match" for the emoji the user is LOOKING at (`shown_json`, the dialog's sasEmoji) in
/// flow `inc`. If the comparison was replaced after those were shown, nothing is confirmed (S5).
pub async fn confirm(inc: u64, shown_json: &str) -> Confirm {
    let (sas, gen) = {
        let a = lock();
        if a.inc != inc {
            return Confirm::NoSas;
        }
        match a.sas.clone() {
            Some(s) => (s, a.gen),
            None => return Confirm::NoSas,
        }
    };
    let now = sas.emoji().map(|e| emoji_json(&e));
    if now.as_deref() != Some(shown_json) {
        eprintln!("[sas] confirm refused: the emoji on screen are not the active comparison's");
        return Confirm::Stale(now);
    }
    // Replaced between the look-up and here? Then this MAC would belong to a dead comparison.
    if !current_sas(inc, gen) {
        eprintln!("[sas] confirm refused: the comparison was replaced a moment ago");
        return Confirm::Stale(None);
    }
    match sas.confirm().await {
        Ok(()) => Confirm::Sent,
        Err(e) => Confirm::Failed(gen, e.to_string()),
    }
}

/// A local failure of flow `inc` the dialog shows as cancelled (e.g. "Could not accept: …"): end
/// the flow and cancel it on the wire. No-op if the flow has ended already.
pub fn fail_flow(inc: u64, note: &str) {
    fail(inc, Caller::Other, note);
}

/// Confirming comparison `gen` of flow `inc` failed: end the flow — unless that comparison has
/// been replaced meanwhile (then the replacement carries on).
pub fn fail_confirm(inc: u64, gen: u64, note: &str) {
    fail(inc, Caller::Sas(gen), note);
}
