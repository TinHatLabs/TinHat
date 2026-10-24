//! TINHAT_SASTEST — headless two-device interactive (emoji/SAS) verification harness.
//!
//! Both roles run in their own process with their own HOME / XDG_DATA_HOME (headless_login
//! WIPES the store under the data dir — never point it at a real profile).
//!
//! Since 2026-09-25 the Tin Hat side runs the SHIPPED driver (sas_driver.rs — the same code the
//! app runs, through a logging sink instead of the Qt one), not a hand copy. The other side can
//! play the clients we meet in the field (TINHAT_SASTEST_PEER), so the Element Classic failure
//! ("the screen just goes black") is reproduced and the fix asserted:
//!
//! Roles (TINHAT_SASTEST=…):
//!   newdev — the device being verified = the REQUESTER, i.e. Tin Hat. Sends the request to the
//!            device TINHAT_SASTEST_TARGET (the way Tin Hat Web's sessions list does; the test
//!            accounts have no scriptable cross-signed device) or, without a target, to every
//!            cross-signed own device (the app's path; needs a vouch with TINHAT_SASTEST_BOOTSTRAP=1
//!            on a scratch account). Then sas_driver::begin(…) with
//!            TINHAT_SASTEST_POLICY=new (default, the fix) | old (the 10.6.0 rules: start at once on
//!            Ready, SDK-default methods, no guard). Presses "They match" for the emoji the sink
//!            was shown (sas_driver::confirm), and "Start emoji comparison" after
//!            TINHAT_SASTEST_MANUAL_MS when offered.
//!   vouch  — the already-signed-in device = the VERIFIER, as persona TINHAT_SASTEST_PEER:
//!            tinhat-new       the shipped driver as verifier ([m.sas.v1], never starts)
//!            tinhat-old       the same with the 10.6.0 accept() ([m.sas.v1, m.reciprocate.v1])
//!            elementx         [m.sas.v1]; starts right after its ready (glare → SDK tie-break)
//!            classic:<ms>     Element Classic 1.11.39 (iOS): [sas, qr.show, qr.scan, reciprocate];
//!                             its crypto auto-accepts a start, but its UI never shows those emoji;
//!                             at <ms> its user taps "Can't scan?" → start_sas UNCONDITIONALLY
//!                             (on a Transitioned request the SDK sends start + 2 cancels = the
//!                             field trace); confirms only a comparison that exists after the tap
//!            classic-ready:<ms>  Classic whose chooser is listening: taps at <ms> only while the
//!                             request is still Ready, otherwise shows/adopts the requester's start
//!            elementweb:<ms>  [sas, qr.show, reciprocate]; starts at <ms> only while Ready, else adopts
//!            never            [sas, qr.show, reciprocate]; never starts, adopts a start
//!   reap   — delete leftover harness devices ("sastest-desk …" display names, plus the ids in
//!            TINHAT_SASTEST_REAP_IDS) with the account password; exits.
//!
//! Env: TINHAT_HS, TINHAT_USER, TINHAT_PASS; TINHAT_DEVICE_ID pins this side's device id (the SDK's start
//! tie-break keeps the lexicographically smaller device's start — run both orders);
//! TINHAT_SASTEST_EXPECT=pass|fail (the verdict is PASS when the outcome matches: pass = the
//! comparison completed); TINHAT_SASTEST_CONFIRM_MS (default 400), TINHAT_SASTEST_ACCEPT_MS (vouch,
//! default 800), TINHAT_SASTEST_SUSPEND=<at_ms>:<dur_ms> (newdev: hold sync that long after the
//! request went out — iOS-style catch-up where ready + start land in one batch),
//! TINHAT_SASTEST_WALL (s, default 120).
//! Output: `[sastest] OUTCOME: …` and `[sastest] VERDICT: PASS|FAIL`; exit 0/1. Every created
//! device logs itself out at the end.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use matrix_sdk::encryption::verification::{SasState, Verification, VerificationRequest, VerificationRequestState};
use matrix_sdk::ruma::events::key::verification::VerificationMethod;
use matrix_sdk::Client;

use crate::sas_driver::{self, Rules, SasUi};

static T0: OnceLock<Instant> = OnceLock::new();
fn ms() -> u128 {
    T0.get_or_init(Instant::now).elapsed().as_millis()
}
macro_rules! say {
    ($($t:tt)*) => { eprintln!("[sastest {:>6}ms] {}", ms(), format!($($t)*)) };
}

fn env_ms(k: &str, d: u64) -> u64 {
    std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}

pub fn run(role: &str) -> ! {
    let _ = ms();
    let wall = env_ms("TINHAT_SASTEST_WALL", 120);
    let expect_pass = std::env::var("TINHAT_SASTEST_EXPECT").map(|v| v != "fail").unwrap_or(true);
    let ok = crate::bridge::runtime().block_on(async {
        let client_slot: Arc<Mutex<Option<Client>>> = Arc::new(Mutex::new(None));
        let outcome = match tokio::time::timeout(Duration::from_secs(wall), run_role(role, client_slot.clone())).await {
            Ok(o) => o,
            Err(_) => Outcome::Stalled(format!("wall timeout after {wall}s")),
        };
        say!("OUTCOME: {}", outcome.describe());
        // Log this device out whatever happened (the server deletes it) — no leftovers.
        let client = client_slot.lock().unwrap().take();
        if let Some(c) = client {
            match tokio::time::timeout(Duration::from_secs(20), c.matrix_auth().logout()).await {
                Ok(Ok(_)) => say!("logged out (device deleted)"),
                Ok(Err(e)) => say!("logout failed: {e} — run TINHAT_SASTEST=reap"),
                Err(_) => say!("logout timed out — run TINHAT_SASTEST=reap"),
            }
        }
        match outcome {
            Outcome::Tool(_) => false,
            Outcome::Completed if expect_pass => true,
            Outcome::Completed => false,
            _ if expect_pass => false,
            _ => true,
        }
    });
    say!("VERDICT: {} (expected {})", if ok { "PASS" } else { "FAIL" }, if expect_pass { "pass" } else { "fail" });
    // _exit: skip C++ static destructors (same reason as the other harnesses).
    unsafe { libc::_exit(if ok { 0 } else { 1 }) };
}

enum Outcome {
    Completed,
    Cancelled(String),
    Stalled(String),
    /// The harness itself could not run (login, reap, setup) — never a PASS.
    Tool(String),
}
impl Outcome {
    fn describe(&self) -> String {
        match self {
            Outcome::Completed => "completed".into(),
            Outcome::Cancelled(n) => format!("cancelled — {n}"),
            Outcome::Stalled(n) => format!("stalled — {n}"),
            Outcome::Tool(n) => format!("harness error — {n}"),
        }
    }
}

// ---- The shipped driver's sink: log + hand the events to the scripted "user" ----------------

enum Ev {
    State(String, String),
    CanStart(bool),
    Emoji(String),
    Done,
}

/// Like the Qt sink: terminal writes apply while the flow is live, progress writes only while it
/// is also open; an applied write records the flow the "screen" shows (commands name that one).
struct HarnessUi(tokio::sync::mpsc::UnboundedSender<Ev>);
static SHOWN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
fn shown_inc() -> u64 {
    SHOWN.load(std::sync::atomic::Ordering::SeqCst)
}
impl HarnessUi {
    fn gate(inc: u64, terminal: bool, what: &str) -> bool {
        let ok = if terminal { sas_driver::is_live(inc) } else { sas_driver::is_open(inc) };
        if ok {
            SHOWN.store(inc, std::sync::atomic::Ordering::SeqCst);
        } else {
            say!("ui: DROPPED a stale write of flow #{inc}: {what}");
        }
        ok
    }
    fn live(inc: u64, what: &str) -> bool {
        Self::gate(inc, false, what)
    }
}
impl SasUi for HarnessUi {
    fn arrived(&self, inc: u64) {
        if Self::live(inc, "arrived") {
            say!("ui: incoming request — state=waiting note={:?}", crate::sas_policy::NOTE_INCOMING_ARRIVED);
        }
    }
    fn set_state(&self, inc: u64, state: &str, note: &str) {
        if Self::gate(inc, state == "cancelled" || state == "done", state) {
            say!("ui: state={state} note={note:?}");
            let _ = self.0.send(Ev::State(state.into(), note.into()));
        }
    }
    fn set_peer(&self, inc: u64, peer: &str) {
        if Self::live(inc, "peer") {
            say!("ui: peer={peer:?}");
        }
    }
    fn set_can_start(&self, inc: u64, can: bool) {
        if Self::gate(inc, !can, "canStart") {
            say!("ui: canStart={can}");
            let _ = self.0.send(Ev::CanStart(can));
        }
    }
    fn show_emoji(&self, inc: u64, emoji_json: &str, note: &str) {
        if Self::live(inc, "emoji") {
            say!("ui: state=emoji {emoji_json} note={note:?}");
            let _ = self.0.send(Ev::Emoji(emoji_json.into()));
        }
    }
    fn sas_done(&self, inc: u64, note: &str) {
        if Self::gate(inc, true, "done") {
            say!("ui: state=done note={note:?}");
            let _ = self.0.send(Ev::Done);
        }
    }
}

/// The scripted user in front of the shipped driver: "They match" for exactly the emoji on
/// screen, "Start emoji comparison" after `manual_ms` when offered. Returns the outcome.
async fn user_loop(mut rx: tokio::sync::mpsc::UnboundedReceiver<Ev>, manual_ms: Option<u64>) -> Outcome {
    let confirm_ms = env_ms("TINHAT_SASTEST_CONFIRM_MS", 400);
    let mut outcome: Option<Outcome> = None;
    let mut linger_until: Option<Instant> = None;
    loop {
        let next = match linger_until {
            Some(t) => match tokio::time::timeout(t.saturating_duration_since(Instant::now()), rx.recv()).await {
                Ok(e) => e,
                Err(_) => break,
            },
            None => rx.recv().await,
        };
        let Some(ev) = next else { break };
        match ev {
            Ev::Emoji(json) => {
                let inc = shown_inc();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(confirm_ms)).await;
                    match sas_driver::confirm(inc, &json).await {
                        sas_driver::Confirm::Sent => say!("user: \"They match\" → confirmed"),
                        sas_driver::Confirm::NoSas => say!("user: \"They match\" → no comparison any more"),
                        sas_driver::Confirm::Stale(_) => say!("user: \"They match\" → REFUSED (stale emoji)"),
                        sas_driver::Confirm::Failed(gen, e) => {
                            say!("user: \"They match\" → failed: {e}");
                            sas_driver::fail_confirm(inc, gen, &format!("Could not confirm: {e}"));
                        }
                    }
                });
            }
            Ev::CanStart(true) => {
                if let Some(ms) = manual_ms {
                    let inc = shown_inc();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(ms)).await;
                        say!("user: taps \"Start emoji comparison\"");
                        sas_driver::start_now(inc).await;
                    });
                }
            }
            Ev::CanStart(false) => {}
            Ev::Done => {
                if outcome.is_none() {
                    outcome = Some(Outcome::Completed);
                    linger_until = Some(Instant::now() + Duration::from_secs(3));
                }
            }
            Ev::State(state, note) => match state.as_str() {
                "done" if outcome.is_none() => {
                    outcome = Some(Outcome::Completed);
                    linger_until = Some(Instant::now() + Duration::from_secs(3));
                }
                "cancelled" => {
                    if matches!(outcome, Some(Outcome::Completed)) {
                        say!("NOTE: a cancel arrived after done: {note:?}");
                    } else if outcome.is_none() {
                        outcome = Some(Outcome::Cancelled(note));
                        linger_until = Some(Instant::now() + Duration::from_secs(3));
                    }
                }
                _ => {}
            },
        }
    }
    outcome.unwrap_or_else(|| Outcome::Stalled("the driver went quiet".into()))
}

/// After a flow: every driver and watcher task must be gone (S5: no orphaned watchers).
async fn assert_no_live_tasks() -> bool {
    for _ in 0..20 {
        if sas_driver::live_tasks() == (0, 0) {
            say!("tasks: no driver/watcher left alive");
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (d, w) = sas_driver::live_tasks();
    say!("LEAK: {d} request driver(s) and {w} SAS watcher(s) still alive after the flow");
    false
}

// ---- Sync loop (same shape as the app's: manual sync_once) ----------------------------------

#[derive(Default)]
struct SyncCtl {
    /// SUSPEND window (absolute), set once the request is out.
    hold: Option<(Instant, Instant)>,
}

fn to_device_summary(v: &serde_json::Value) -> String {
    // Names/codes only — never keys, MACs or commitments.
    let c = v.get("content");
    let get = |k: &str| c.and_then(|c| c.get(k)).cloned();
    let mut parts = Vec::new();
    if let Some(serde_json::Value::Array(ms)) = get("methods") {
        let names: Vec<String> = ms.iter().filter_map(|m| m.as_str().map(str::to_owned)).collect();
        parts.push(format!("methods=[{}]", names.join(",")));
    }
    if let Some(m) = get("method").and_then(|m| m.as_str().map(str::to_owned)) {
        parts.push(format!("method={m}"));
    }
    if let Some(code) = get("code").and_then(|m| m.as_str().map(str::to_owned)) {
        parts.push(format!("code={code}"));
    }
    if let Some(d) = get("from_device").and_then(|m| m.as_str().map(str::to_owned)) {
        parts.push(format!("from_device={d}"));
    }
    parts.join(" ")
}

fn spawn_sync(client: Client, ctl: Arc<Mutex<SyncCtl>>, req_tx: tokio::sync::mpsc::UnboundedSender<(String, String)>) {
    // The app long-polls 30 s. A persona that must flush SDK-queued messages promptly (Classic's
    // two cancels go out through the outgoing-request queue, i.e. on the next sync) sets
    // TINHAT_SASTEST_SYNC_MS; the SUSPEND test needs short polls to hold the ready and start back.
    let poll = std::env::var("TINHAT_SASTEST_SYNC_MS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(
        if std::env::var_os("TINHAT_SASTEST_SUSPEND").is_some() { 1000 } else { 30_000 },
    );
    tokio::spawn(async move {
        let settings = crate::bridge::sync_settings().timeout(Duration::from_millis(poll));
        loop {
            let hold = ctl.lock().unwrap().hold;
            if let Some((from, until)) = hold {
                let now = Instant::now();
                if now >= from && now < until {
                    say!("sync: HELD for {} ms (catch-up test)", (until - now).as_millis());
                    tokio::time::sleep(until - now).await;
                    say!("sync: released");
                }
            }
            match client.sync_once(settings.clone()).await {
                Ok(response) => {
                    for pe in &response.to_device {
                        use matrix_sdk::deserialized_responses::ProcessedToDeviceEvent as P;
                        let variant = match pe {
                            P::Decrypted { .. } => "decrypted",
                            P::UnableToDecrypt { .. } => "UTD",
                            P::PlainText(_) => "plain",
                            P::Invalid(_) => "invalid",
                        };
                        let raw = pe.as_raw();
                        let v = raw.deserialize_as::<serde_json::Value>().ok();
                        let ty = v.as_ref().and_then(|v| v.get("type")).and_then(|t| t.as_str()).unwrap_or("?").to_string();
                        let sender = v.as_ref().and_then(|v| v.get("sender")).and_then(|s| s.as_str()).unwrap_or("?").to_string();
                        if ty.starts_with("m.key.verification.") {
                            say!("td <- {variant} {ty} {}", v.as_ref().map(to_device_summary).unwrap_or_default());
                        }
                        if ty == "m.key.verification.request" {
                            let txn = v
                                .as_ref()
                                .and_then(|v| v.get("content"))
                                .and_then(|c| c.get("transaction_id"))
                                .and_then(|t| t.as_str())
                                .map(str::to_owned);
                            if let Some(txn) = txn {
                                let _ = req_tx.send((sender, txn));
                            }
                        }
                    }
                }
                Err(e) => {
                    say!("sync error: {e}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    });
}

async fn run_role(role: &str, slot: Arc<Mutex<Option<Client>>>) -> Outcome {
    let hs = match crate::bridge::env_homeserver() {
        Ok(h) => h,
        Err(e) => return Outcome::Tool(e),
    };
    let Ok(user) = std::env::var("TINHAT_USER") else { return Outcome::Tool("TINHAT_USER unset".into()) };
    let Ok(pass) = std::env::var("TINHAT_PASS") else { return Outcome::Tool("TINHAT_PASS unset".into()) };

    let client = match crate::bridge::headless_login(&hs, &user, &pass).await {
        Ok(c) => c,
        Err(e) => return Outcome::Tool(format!("login failed: {e}")),
    };
    *slot.lock().unwrap() = Some(client.clone());
    let own = client.user_id().unwrap().to_owned();
    let me = client.device_id().unwrap().to_owned();
    let name = format!("sastest-desk {role}");
    let _ = client.rename_device(&me, &name).await;
    say!("role={role} user={own} device={me} name={name:?}");

    if role == "reap" {
        return reap(&client, &own, &pass).await;
    }

    let ctl = Arc::new(Mutex::new(SyncCtl::default()));
    let (req_tx, req_rx) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();
    spawn_sync(client.clone(), ctl.clone(), req_tx);

    match role {
        "vouch" => vouch(&client, &own, &pass, req_rx).await,
        "newdev" => newdev(&client, &own, ctl).await,
        other => Outcome::Tool(format!("unknown role {other:?} (want vouch|newdev|reap)")),
    }
}

async fn reap(client: &Client, own: &matrix_sdk::ruma::UserId, pass: &str) -> Outcome {
    use matrix_sdk::ruma::api::client::uiaa;
    let me = client.device_id().unwrap().to_owned();
    let extra: Vec<String> = std::env::var("TINHAT_SASTEST_REAP_IDS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let devs = match client.devices().await {
        Ok(r) => r.devices,
        Err(e) => return Outcome::Tool(format!("list devices: {e}")),
    };
    let ids: Vec<matrix_sdk::ruma::OwnedDeviceId> = devs
        .iter()
        .filter(|d| d.device_id != me)
        .filter(|d| {
            d.display_name.as_deref().is_some_and(|n| n.starts_with("sastest-desk"))
                || extra.iter().any(|x| x == d.device_id.as_str())
        })
        .map(|d| d.device_id.clone())
        .collect();
    say!("reap: {} leftover device(s): {:?}", ids.len(), ids.iter().map(|d| d.as_str()).collect::<Vec<_>>());
    if ids.is_empty() {
        return Outcome::Completed;
    }
    if let Err(e) = client.delete_devices(&ids, None).await {
        let Some(info) = e.as_uiaa_response() else { return Outcome::Tool(format!("delete: {e}")) };
        let mut pw = uiaa::Password::new(uiaa::UserIdentifier::from(own.to_owned()), pass.to_owned());
        pw.session = info.session.clone();
        if let Err(e) = client.delete_devices(&ids, Some(uiaa::AuthData::Password(pw))).await {
            return Outcome::Tool(format!("delete with password: {e}"));
        }
    }
    say!("reap: deleted {}", ids.len());
    Outcome::Completed
}

// ---- newdev: Tin Hat as the requester (the SHIPPED driver) ----------------------------------

async fn newdev(client: &Client, own: &matrix_sdk::ruma::UserId, ctl: Arc<Mutex<SyncCtl>>) -> Outcome {
    let rules = match std::env::var("TINHAT_SASTEST_POLICY").as_deref() {
        Ok("old") => Rules::Shipped1060,
        _ => Rules::Current,
    };
    let manual_ms = std::env::var("TINHAT_SASTEST_MANUAL_MS").ok().and_then(|v| v.parse().ok());
    let target = std::env::var("TINHAT_SASTEST_TARGET").ok().filter(|s| !s.is_empty());
    say!("newdev: rules={rules:?} target={target:?} manual_ms={manual_ms:?}");

    let req: VerificationRequest = if let Some(target) = target {
        let dev_id: matrix_sdk::ruma::OwnedDeviceId = target.as_str().into();
        // Wait until the target device's keys are known (the vouch may still be signing in).
        let device = loop {
            match client.encryption().get_device(own, &dev_id).await {
                Ok(Some(d)) => break d,
                Ok(None) => {
                    say!("newdev: waiting for device {target} to appear…");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Err(e) => return Outcome::Tool(format!("get_device: {e}")),
            }
        };
        let r = match rules {
            Rules::Current => device.request_verification_with_methods(crate::sas_policy::our_methods()).await,
            Rules::Shipped1060 => device.request_verification().await,
        };
        match r {
            Ok(r) => r,
            Err(e) => return Outcome::Tool(format!("request_verification: {e}")),
        }
    } else {
        // The app's own path: every cross-signed own device (needs a vouch with BOOTSTRAP=1).
        let me = client.device_id().unwrap().to_owned();
        loop {
            if let Ok(devs) = client.encryption().get_user_devices(own).await {
                if devs.devices().any(|d| d.device_id() != me && d.is_cross_signed_by_owner()) {
                    break;
                }
            }
            say!("newdev: waiting for a cross-signed own device…");
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        let identity = loop {
            match client.encryption().get_user_identity(own).await {
                Ok(Some(i)) => break i,
                Ok(None) => tokio::time::sleep(Duration::from_secs(2)).await,
                Err(e) => return Outcome::Tool(format!("get_user_identity: {e}")),
            }
        };
        let r = match rules {
            Rules::Current => identity.request_verification_with_methods(crate::sas_policy::our_methods()).await,
            Rules::Shipped1060 => identity.request_verification().await,
        };
        match r {
            Ok(r) => r,
            Err(e) => return Outcome::Tool(format!("request_verification: {e}")),
        }
    };
    say!("newdev: request SENT flow_id={} self={} we_started={}", req.flow_id(), req.is_self_verification(), req.we_started());
    if let Ok(s) = std::env::var("TINHAT_SASTEST_SUSPEND") {
        if let Some((a, d)) = s.split_once(':') {
            let (a, d): (u64, u64) = (a.parse().unwrap_or(0), d.parse().unwrap_or(0));
            let now = Instant::now();
            ctl.lock().unwrap().hold = Some((now + Duration::from_millis(a), now + Duration::from_millis(a + d)));
            say!("newdev: sync will be held {a}..{} ms from now", a + d);
        }
    }

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let (inc, _) = sas_driver::reserve();
    SHOWN.store(inc, std::sync::atomic::Ordering::SeqCst);
    if !sas_driver::begin_outgoing(req.clone(), inc, Arc::new(HarnessUi(tx)), rules) {
        return Outcome::Tool("begin_outgoing refused a fresh attempt".into());
    }
    // TINHAT_SASTEST_CANCEL_MS: the requester gives up this long after asking (S8 on the other side:
    // its sheet must show cancelled, not a stale Accept).
    if let Some(cancel_ms) = std::env::var("TINHAT_SASTEST_CANCEL_MS").ok().and_then(|v| v.parse::<u64>().ok()) {
        tokio::time::sleep(Duration::from_millis(cancel_ms)).await;
        say!("user: taps Cancel (before the other side accepted)");
        sas_driver::end_by_user().cancel_on_wire(false).await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        let _ = assert_no_live_tasks().await;
        return Outcome::Cancelled("the requester cancelled (TINHAT_SASTEST_CANCEL_MS)".into());
    }
    let outcome = user_loop(rx, manual_ms).await;
    let tasks_ok = assert_no_live_tasks().await;
    if matches!(outcome, Outcome::Completed) && std::env::var("TINHAT_SASTEST_TARGET").map(|t| t.is_empty()).unwrap_or(true) {
        // App path only: the private cross-signing keys must gossip over too.
        for i in 0..30 {
            match client.encryption().cross_signing_status().await {
                Some(s) if s.has_master && s.has_self_signing && s.has_user_signing => {
                    say!("newdev: fully verified after {}s", i * 2);
                    break;
                }
                _ => tokio::time::sleep(Duration::from_secs(2)).await,
            }
        }
    }
    if !tasks_ok {
        return Outcome::Tool("driver/watcher tasks leaked".into());
    }
    outcome
}

// ---- vouch: the other device, as a persona -------------------------------------------------

#[derive(Clone, Debug)]
enum Persona {
    TinHat(Rules),
    Foreign {
        name: String,
        methods: Vec<VerificationMethod>,
        /// When the persona's user taps its own "Verify by emoji" (ms after accept).
        tap_ms: Option<u64>,
        /// Tap only while the request is still Ready (a listening chooser); otherwise start
        /// unconditionally (Classic's chooser that never saw the requester's start).
        tap_only_if_ready: bool,
        /// Confirm only a comparison that exists after its own tap (the user never saw the rest).
        confirm_after_tap_only: bool,
        /// Send a raw m.key.verification.cancel with this code, this long after its ready
        /// (screenshots of the S6 notes; e.g. m.unexpected_message, the field code).
        raw_cancel: Option<(String, u64)>,
    },
}

fn parse_persona(s: &str) -> Option<Persona> {
    use VerificationMethod as M;
    let (kind, arg) = s.split_once(':').map(|(a, b)| (a, Some(b))).unwrap_or((s, None));
    let ms = |d: u64| arg.and_then(|a| a.parse().ok()).unwrap_or(d);
    let classic = vec![M::SasV1, M::QrCodeShowV1, M::QrCodeScanV1, M::ReciprocateV1];
    let web = vec![M::SasV1, M::QrCodeShowV1, M::ReciprocateV1];
    Some(match kind {
        "tinhat-new" => Persona::TinHat(Rules::Current),
        "tinhat-old" => Persona::TinHat(Rules::Shipped1060),
        "elementx" => Persona::Foreign {
            name: s.into(),
            methods: vec![M::SasV1],
            tap_ms: Some(ms(0)),
            tap_only_if_ready: true,
            confirm_after_tap_only: false,
            raw_cancel: None,
        },
        "classic" => Persona::Foreign {
            name: s.into(),
            methods: classic,
            tap_ms: Some(ms(3000)),
            tap_only_if_ready: false,
            confirm_after_tap_only: true,
            raw_cancel: None,
        },
        "classic-ready" => Persona::Foreign {
            name: s.into(),
            methods: classic,
            tap_ms: Some(ms(0)),
            tap_only_if_ready: true,
            confirm_after_tap_only: false,
            raw_cancel: None,
        },
        "elementweb" => Persona::Foreign {
            name: s.into(),
            methods: web,
            tap_ms: Some(ms(2000)),
            tap_only_if_ready: true,
            confirm_after_tap_only: false,
            raw_cancel: None,
        },
        "never" => Persona::Foreign {
            name: s.into(),
            methods: web,
            tap_ms: None,
            tap_only_if_ready: true,
            confirm_after_tap_only: false,
            raw_cancel: None,
        },
        // cancel:<code>:<ms> — SAS-only, adopts a start, never confirms, cancels with <code>.
        "cancel" => {
            let (code, at) = arg.and_then(|a| a.rsplit_once(':')).unwrap_or(("m.unexpected_message", "4000"));
            Persona::Foreign {
                name: s.into(),
                methods: vec![M::SasV1],
                tap_ms: None,
                tap_only_if_ready: true,
                confirm_after_tap_only: true,
                raw_cancel: Some((code.to_string(), at.parse().unwrap_or(4000))),
            }
        }
        _ => return None,
    })
}

async fn vouch(
    client: &Client,
    own: &matrix_sdk::ruma::UserId,
    pass: &str,
    mut req_rx: tokio::sync::mpsc::UnboundedReceiver<(String, String)>,
) -> Outcome {
    let persona_s = std::env::var("TINHAT_SASTEST_PEER").unwrap_or_else(|_| "tinhat-new".into());
    let Some(persona) = parse_persona(&persona_s) else {
        return Outcome::Tool(format!("unknown TINHAT_SASTEST_PEER {persona_s:?}"));
    };
    say!("vouch: persona {persona_s} = {persona:?}");
    if std::env::var_os("TINHAT_SASTEST_BOOTSTRAP").is_some() {
        // Legacy scratch-account mode: become a device that can vouch via cross-signing.
        use matrix_sdk::ruma::api::client::uiaa;
        let auth = uiaa::AuthData::Password(uiaa::Password::new(uiaa::UserIdentifier::from(own.to_owned()), pass.to_owned()));
        if let Err(e) = client.encryption().bootstrap_cross_signing_if_needed(Some(auth)).await {
            return Outcome::Tool(format!("cross-signing bootstrap failed: {e}"));
        }
        say!("vouch: cross-signing bootstrapped");
    }
    say!("vouch: waiting for a request…");
    while let Some((sender, txn)) = req_rx.recv().await {
        let Ok(who) = matrix_sdk::ruma::UserId::parse(sender.as_str()) else { continue };
        let mut req = None;
        for _ in 0..20 {
            if let Some(r) = client.encryption().get_verification_request(&who, &txn).await {
                req = Some(r);
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let Some(req) = req else {
            say!("vouch: get_verification_request -> None (txn={txn})");
            continue;
        };
        if !req.is_self_verification() {
            continue;
        }
        let accept_ms = env_ms("TINHAT_SASTEST_ACCEPT_MS", 800);
        let outcome = match &persona {
            Persona::TinHat(rules) => {
                // Exactly the app: driven from arrival (S8), the user taps Accept a moment later.
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                let inc = sas_driver::begin_incoming(req.clone(), Arc::new(HarnessUi(tx)), *rules);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(accept_ms)).await;
                    say!("user: taps Accept");
                    if let Err(e) = sas_driver::accept(inc).await {
                        say!("vouch: accept failed: {e}");
                    }
                });
                let o = user_loop(rx, None).await;
                if !assert_no_live_tasks().await {
                    return Outcome::Tool("driver/watcher tasks leaked".into());
                }
                o
            }
            Persona::Foreign { .. } => foreign_peer(client, req, &persona, accept_ms).await,
        };
        if matches!(outcome, Outcome::Completed) && std::env::var_os("TINHAT_SASTEST_BOOTSTRAP").is_some() {
            // Stay online to answer the new device's m.secret.request gossip.
            say!("vouch: lingering 30s to serve secret gossip");
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
        return outcome;
    }
    Outcome::Stalled("sync ended before any request arrived".into())
}

fn sas_name(s: &SasState) -> &'static str {
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

/// A non-Tin-Hat verifier, scripted from its upstream source (see the module doc).
async fn foreign_peer(client: &Client, req: VerificationRequest, persona: &Persona, accept_ms: u64) -> Outcome {
    let Persona::Foreign { name, methods, tap_ms, tap_only_if_ready, confirm_after_tap_only, raw_cancel } = persona.clone()
    else {
        return Outcome::Tool("not a foreign persona".into());
    };
    let confirm_ms = env_ms("TINHAT_SASTEST_CONFIRM_MS", 400);
    tokio::time::sleep(Duration::from_millis(accept_ms)).await;
    let names: Vec<&str> = methods.iter().map(|m| m.as_str()).collect();
    say!("{name}: user taps Accept → ready with [{}]", names.join(","));
    if let Err(e) = req.accept_with_methods(methods.clone()).await {
        return Outcome::Tool(format!("accept_with_methods: {e}"));
    }
    if let Some((code, at)) = raw_cancel {
        let (req2, client2, name2) = (req.clone(), client.clone(), name.clone());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(at)).await;
            let other = match req2.state() {
                VerificationRequestState::Ready { other_device_data, .. } => other_device_data.device_id().to_owned(),
                VerificationRequestState::Transitioned { verification: Verification::SasV1(sas), .. } => {
                    sas.other_device().device_id().to_owned()
                }
                _ => {
                    say!("{name2}: raw cancel skipped (request no longer live)");
                    return;
                }
            };
            say!("{name2}: sends a raw m.key.verification.cancel code={code}");
            if let Err(e) = send_raw_cancel(&client2, req2.own_user_id(), &other, req2.flow_id(), &code).await {
                say!("{name2}: raw cancel failed: {e}");
            }
        });
    }
    let tapped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    if let Some(t) = tap_ms {
        let (req2, tapped2, name2) = (req.clone(), tapped.clone(), name.clone());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(t)).await;
            let st = req2.state();
            let is_ready = matches!(st, VerificationRequestState::Ready { .. });
            if tap_only_if_ready && !is_ready {
                say!("{name2}: its user would tap now, but a comparison is already on its screen — adopts it");
                return;
            }
            tapped2.store(true, std::sync::atomic::Ordering::SeqCst);
            let st_name = match st {
                VerificationRequestState::Ready { .. } => "Ready",
                VerificationRequestState::Transitioned { .. } => "Transitioned",
                VerificationRequestState::Done => "Done",
                VerificationRequestState::Cancelled(_) => "Cancelled",
                _ => "other",
            };
            say!("{name2}: user taps \"Verify by emoji\" → start_sas on a {st_name} request");
            match req2.start_sas().await {
                Ok(Some(_)) => say!("{name2}: its start went out"),
                Ok(None) => say!("{name2}: start_sas -> None"),
                Err(e) => say!("{name2}: start_sas failed: {e}"),
            }
        });
    }
    let (otx, mut orx) = tokio::sync::mpsc::unbounded_channel::<Outcome>();
    let mut watcher: Option<tokio::task::JoinHandle<()>> = None;
    let mut changes = req.changes();
    let mut pending = Some(req.state());
    let outcome = loop {
        let st = tokio::select! {
            o = orx.recv() => { if let Some(o) = o { break o } else { continue } }
            s = async { match pending.take() { Some(s) => Some(s), None => changes.next().await } } => s,
        };
        let Some(st) = st else { break Outcome::Stalled("request stream ended".into()) };
        match st {
            VerificationRequestState::Transitioned { verification: Verification::SasV1(sas), .. } => {
                say!("{name}: request -> Transitioned (a comparison exists)");
                if let Some(h) = watcher.take() {
                    h.abort();
                }
                let (otx2, tapped2, name2) = (otx.clone(), tapped.clone(), name.clone());
                let seen_after_tap = tapped.load(std::sync::atomic::Ordering::SeqCst);
                watcher = Some(tokio::spawn(async move {
                    let mut ch = sas.changes();
                    let mut p = Some(sas.state());
                    loop {
                        let s = match p.take() {
                            Some(s) => s,
                            None => match ch.next().await {
                                Some(s) => s,
                                None => break,
                            },
                        };
                        say!("{name2}: sas -> {}", sas_name(&s));
                        match s {
                            // Its crypto auto-accepts a start from the other side (MatrixSDK V2 /
                            // Element X / Element Web all do).
                            SasState::Started { .. } => {
                                let _ = sas.accept().await;
                            }
                            SasState::KeysExchanged { emojis, .. } => {
                                let shown = emojis
                                    .map(|e| e.emojis.iter().map(|x| x.description).collect::<Vec<_>>().join(" "))
                                    .unwrap_or_default();
                                let visible = !confirm_after_tap_only
                                    || seen_after_tap
                                    || tapped2.load(std::sync::atomic::Ordering::SeqCst);
                                if visible {
                                    say!("{name2}: shows emoji [{shown}] — user taps \"They match\"");
                                    tokio::time::sleep(Duration::from_millis(confirm_ms)).await;
                                    if let Err(e) = sas.confirm().await {
                                        say!("{name2}: confirm failed: {e}");
                                    }
                                } else {
                                    say!("{name2}: keys exchanged but its UI never shows these emoji (chooser not listening)");
                                }
                            }
                            SasState::Done { .. } => {
                                let _ = otx2.send(Outcome::Completed);
                                break;
                            }
                            SasState::Cancelled(info) => {
                                say!("{name2}: comparison cancelled code={}", info.cancel_code().as_str());
                                break;
                            }
                            _ => {}
                        }
                    }
                }));
            }
            VerificationRequestState::Transitioned { .. } => say!("{name}: non-SAS transition"),
            VerificationRequestState::Done => break Outcome::Completed,
            VerificationRequestState::Cancelled(info) => {
                break Outcome::Cancelled(format!("code={} by_us={}", info.cancel_code().as_str(), info.cancelled_by_us()))
            }
            VerificationRequestState::Ready { .. } => say!("{name}: request -> Ready"),
            _ => {}
        }
    };
    // Let a trailing cancel / done land (Classic's cancels come 13 ms after its start).
    tokio::time::sleep(Duration::from_secs(2)).await;
    if let Some(h) = watcher.take() {
        h.abort();
    }
    outcome
}

/// A bare m.key.verification.cancel (to-device, unencrypted like every verification message).
async fn send_raw_cancel(
    client: &Client,
    user: &matrix_sdk::ruma::UserId,
    device: &matrix_sdk::ruma::DeviceId,
    flow: &str,
    code: &str,
) -> Result<(), String> {
    use matrix_sdk::ruma::{
        api::client::to_device::send_event_to_device::v3::Request, events::AnyToDeviceEventContent, serde::Raw,
        to_device::DeviceIdOrAllDevices, TransactionId,
    };
    let content = serde_json::json!({ "transaction_id": flow, "code": code, "reason": "TINHAT_SASTEST raw cancel" });
    let raw: Raw<AnyToDeviceEventContent> =
        Raw::from_json(serde_json::value::to_raw_value(&content).map_err(|e| e.to_string())?);
    let mut msgs = std::collections::BTreeMap::new();
    msgs.entry(user.to_owned())
        .or_insert_with(std::collections::BTreeMap::new)
        .insert(DeviceIdOrAllDevices::DeviceId(device.to_owned()), raw);
    let req = Request::new_raw("m.key.verification.cancel".into(), TransactionId::new(), msgs);
    client.send(req).await.map(|_| ()).map_err(|e| e.to_string())
}
