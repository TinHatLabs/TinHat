//! pad_net.rs — Pad's transport and trust glue (docs/PAD.md "Transport" + "Security rules"; the web's
//! `features/calls/pad/transport.ts`). One [`PadNode`] per signed-in client drives the two state machines of
//! pad_engine.rs with real Olm to-device messages:
//!   * receive: the sync loop hands every to-device event here (bridge.rs, beside the media-key branch). Only a
//!     `dev.tinhat.pad.cmd` / `.state` is read; the trust facts are looked up for EVERY message, nothing cached — the
//!     Olm sender (= the event sender = me), the Curve25519 key bound to the device RECORD (the same glue as
//!     key_intake.rs's `bind_device`), THIS device verified (all three cross-signing private keys), the peer
//!     matrix-sdk-crypto `Device::is_cross_signing_trusted()` (never `is_cross_signed_by_owner`) and not deleted —
//!     and handed to the pure rules. A plaintext / UTD event of those types never reaches them: `not-olm`, dropped.
//!     Another account's message costs no lookup at all.
//!   * send: `encrypt_and_send_raw_to_device` to exactly ONE device, encrypting for the very `Device` that was just
//!     checked (cross-signing trusted, not deleted, this device verified: rule 10 for states, the same for commands),
//!     with the SDK's `OnlyTrustedDevices` as a second guard. One device's messages leave IN ORDER (one outbox per
//!     device, filled under the lock that numbers them): a reply must never arrive after a newer push (dropped as an
//!     old `rseq`), nor a command after a newer one (dropped as an old `seq`). The outbox is bounded, a message that
//!     waited more than [`SEND_MAX_AGE_MS`] is dropped instead of sent late (never "a tap fires minutes later"), a
//!     send that hangs is cut off after [`SEND_TIMEOUT_MS`], and an idle outbox goes away.
//! Nothing here talks to the UI directly: events go out through the node's sink (pad_ctl.rs → QML; pad_lab.rs →
//! the headless test). The sink is called with the receiver's lock held: it must only queue, never call back.
use crate::pad_engine::{self as pe, CEv, Controller, DeviceRecord, Out, Receiver, Snapshot};
use crate::padrules::{self as pr, PadFacts};
use matrix_sdk::deserialized_responses::{AlgorithmInfo, ProcessedToDeviceEvent};
use matrix_sdk::ruma::{OwnedDeviceId, OwnedUserId};
use matrix_sdk::Client;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A message that waited longer than this in its outbox is dropped, not sent late (commands are stale at 8 s).
pub const SEND_MAX_AGE_MS: u128 = 6_000;
/// One send (key claim + encrypt + PUT) is given up after this long.
pub const SEND_TIMEOUT_MS: u64 = 10_000;
/// At most this many messages wait for one device (more = dropped: the controller resends, states are snapshots).
pub const OUTBOX_MAX: usize = 32;
/// An outbox with nothing to send for this long goes away.
pub const OUTBOX_IDLE_MS: u64 = 60_000;

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// A fresh session id: 16 base64url characters from the OS-seeded CSPRNG.
pub fn mint_sid() -> String {
    use rand::Rng;
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut rng = rand::thread_rng();
    (0..16).map(|_| A[rng.gen_range(0..64)] as char).collect()
}

/// An untrusted string for a log line: at most 40 printable ASCII characters (no newlines, no terminal codes).
fn clean(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_graphic()).take(40).collect()
}

/// What a node tells its owner (the UI, or the lab test).
#[derive(Clone, Debug)]
pub enum PadEvent {
    /// receiver: run these commands (`[{do, …}]`, QML PadHost.run) — after `claim(token)` said yes — then
    /// `executed(token)`
    Exec { token: i64, cmds: Value },
    /// receiver: the devices with a live Pad session here changed (the chip)
    Chips(Vec<String>),
    /// controller: the remote view changed ([`PadNode::remote_view`])
    Remote,
    /// controller: the receiver answered command `seq`
    Ack { seq: i64, ok: bool, why: String, op: String },
    /// controller: no answer after the resend (roll back, "Not reaching {device}")
    Failed { seq: i64, op: String },
    /// controller: the session is over ("replaced", "closed", "off", "unreachable", a refused open's code)
    Ended(String),
    /// a decision worth a log line (the lab test reads them; the app's sink drops them: they are on stderr already)
    Log(String),
}

pub type Sink = Arc<dyn Fn(PadEvent) + Send + Sync>;

struct Queued {
    typ: &'static str,
    content: Value,
    at: Instant,
}
impl Queued {
    fn sid(&self) -> &str {
        self.content.get("sid").and_then(|s| s.as_str()).unwrap_or("")
    }
    fn is_close(&self) -> bool {
        self.content.get("op").and_then(|s| s.as_str()) == Some("close")
    }
}

static NEXT_NODE: AtomicU64 = AtomicU64::new(1);

/// One signed-in device's Pad: the receiver and the (at most one) session it drives.
pub struct PadNode {
    pub client: Client,
    /// unique in the process (pad_ctl.rs drops events of a node that is no longer the session's)
    pub id: u64,
    rx: Mutex<Receiver>,
    ctl: Mutex<Option<Controller>>,
    sink: Sink,
    chips: Mutex<Vec<String>>,
    ticking: AtomicBool,
    /// per destination device: its ordered, bounded sender (one task each, one message at a time)
    outbox: Mutex<HashMap<String, tokio::sync::mpsc::Sender<Queued>>>,
    /// last background /keys/query for my account (an unknown device of mine)
    queried_at: Mutex<i64>,
    /// rejection log lines in the current 10 s window (an unauthenticated sender can't flood the log)
    log_budget: Mutex<(i64, u32)>,
    /// sessions this device closed or gave up: their queued commands are not sent any more (their close still is)
    cancelled: Mutex<std::collections::VecDeque<String>>,
}

impl PadNode {
    pub fn new(client: Client, pad_on: bool, sink: Sink) -> Arc<PadNode> {
        Arc::new(PadNode {
            client,
            id: NEXT_NODE.fetch_add(1, Ordering::SeqCst),
            rx: Mutex::new(Receiver::new(pad_on)),
            ctl: Mutex::new(None),
            sink,
            chips: Mutex::new(Vec::new()),
            ticking: AtomicBool::new(false),
            outbox: Mutex::new(HashMap::new()),
            queried_at: Mutex::new(0),
            log_budget: Mutex::new((0, 0)),
            cancelled: Mutex::new(std::collections::VecDeque::new()),
        })
    }

    fn emit(&self, ev: PadEvent) {
        (self.sink)(ev)
    }
    fn log(&self, line: String) {
        eprintln!("[pad] {line}");
        self.emit(PadEvent::Log(line));
    }
    /// A rejection line: at most 20 per 10 s (then one "… more suppressed" line per window).
    fn log_rejection(&self, line: String) {
        let now = now_ms();
        let show = {
            let mut b = self.log_budget.lock().unwrap();
            if now - b.0 > 10_000 {
                *b = (now, 0);
            }
            b.1 += 1;
            b.1
        };
        match show {
            1..=20 => self.log(line),
            21 => self.log("more Pad rejections in the last 10 s suppressed".into()),
            _ => {}
        }
    }
    fn my_ids(&self) -> (String, String) {
        (
            self.client.user_id().map(|u| u.to_string()).unwrap_or_default(),
            self.client.device_id().map(|d| d.to_string()).unwrap_or_default(),
        )
    }

    // ---- trust lookups (every message; nothing cached) ----

    async fn self_verified(&self) -> bool {
        matches!(self.client.encryption().cross_signing_status().await, Some(s) if s.has_master && s.has_self_signing && s.has_user_signing)
    }

    /// My device `device` as the crypto store has it now (None = not on record: a key query starts in the
    /// background, at most every 10 s; the message is not held — the controller resends after 3 s).
    async fn my_device(&self, device: &str) -> Option<matrix_sdk::encryption::identities::Device> {
        let me = self.client.user_id()?.to_owned();
        match self.client.encryption().get_device(&me, &OwnedDeviceId::from(device)).await {
            Ok(Some(d)) => Some(d),
            _ => {
                self.query_my_keys();
                None
            }
        }
    }
    fn record_of(d: &matrix_sdk::encryption::identities::Device) -> DeviceRecord {
        DeviceRecord {
            device: d.device_id().to_string(),
            curve: d.curve25519_key().map(|k| k.to_base64()).unwrap_or_default(),
            // matrix-sdk's wrapper of matrix-sdk-crypto `Device::is_cross_signing_trusted()` (signed by my self-signing
            // key AND my own identity verified) — never `is_cross_signed_by_owner()`
            cross_signing_trusted: d.is_verified_with_cross_signing(),
            deleted: d.is_deleted(),
        }
    }
    /// The device of mine whose Curve25519 key is `curve` (when the SDK named none).
    async fn record_by_curve(&self, curve: &str) -> Option<DeviceRecord> {
        let me = self.client.user_id()?.to_owned();
        let devs = self.client.encryption().get_user_devices(&me).await.ok()?;
        let found = devs.devices().find(|d| d.curve25519_key().map(|k| k.to_base64()).as_deref() == Some(curve)).map(|d| Self::record_of(&d));
        if found.is_none() {
            self.query_my_keys();
        }
        found
    }

    fn query_my_keys(&self) {
        let now = now_ms();
        {
            let mut at = self.queried_at.lock().unwrap();
            if now - *at < 10_000 {
                return;
            }
            *at = now;
        }
        let (c, me) = (self.client.clone(), self.client.user_id().map(|u| u.to_owned()));
        crate::bridge::runtime().spawn(async move {
            if let Some(me) = me {
                let _ = c.encryption().request_user_identity(&me).await;
            }
        });
    }

    /// Rules 1–4's facts for one to-device event (`event` = its JSON: `{type, sender, content}`), looked up now.
    pub async fn facts(&self, pe_: &ProcessedToDeviceEvent, event: &Value) -> PadFacts {
        let (me, my_dev) = self.my_ids();
        let event_sender = event.get("sender").and_then(|s| s.as_str()).unwrap_or("");
        let ProcessedToDeviceEvent::Decrypted { encryption_info, .. } = pe_ else {
            return pe::facts_from(false, "", event_sender, "", None, &me, &my_dev, false);
        };
        let olm_sender = encryption_info.sender.to_string();
        let curve = match &encryption_info.algorithm_info {
            AlgorithmInfo::OlmV1Curve25519AesSha2 { curve25519_public_key_base64 } => curve25519_public_key_base64.clone(),
            _ => String::new(),
        };
        // rules 1–2 need no lookup: another account (or a mismatched sender) costs no store read
        if olm_sender != event_sender || olm_sender != me || curve.is_empty() {
            return pe::facts_from(true, &olm_sender, event_sender, &curve, None, &me, &my_dev, false);
        }
        // the device record LAST: nothing awaits between it and the decision (on_cmd takes the receiver's lock next)
        let verified = self.self_verified().await;
        let record = match encryption_info.sender_device.as_ref() {
            Some(d) => self.my_device(d.as_str()).await.map(|d| Self::record_of(&d)),
            None => self.record_by_curve(&curve).await,
        };
        pe::facts_from(true, &olm_sender, event_sender, &curve, record.as_ref(), &me, &my_dev, verified)
    }

    // ---- the wire ----

    /// One Olm-encrypted to-device message to exactly one device of mine, sent now (no queue, no resend later) —
    /// only when this device is verified and THAT device (the very object encrypted for) is cross-signing trusted
    /// and not deleted (rule 10 for states; the same for commands). The lab gate sends crafted messages with
    /// [`PadNode::send_raw_to`] instead.
    async fn send_checked(&self, device: &str, typ: &str, content: &Value) -> Result<(), String> {
        let (_, my_dev) = self.my_ids();
        if device.is_empty() || device == my_dev {
            return Err("not another device of mine".into());
        }
        if !self.self_verified().await {
            return Err("this device isn't verified".into());
        }
        let d = self.my_device(device).await.ok_or("not on record")?;
        if !d.is_verified_with_cross_signing() || d.is_deleted() || d.curve25519_key().is_none() {
            return Err("not a verified device of mine".into());
        }
        Self::encrypt_send(&self.client, &d, typ, content, matrix_sdk_base::crypto::CollectStrategy::OnlyTrustedDevices).await
    }
    async fn encrypt_send(
        client: &Client,
        d: &matrix_sdk::encryption::identities::Device,
        typ: &str,
        content: &Value,
        strategy: matrix_sdk_base::crypto::CollectStrategy,
    ) -> Result<(), String> {
        let raw: matrix_sdk::ruma::serde::Raw<matrix_sdk::ruma::events::AnyToDeviceEventContent> =
            matrix_sdk::ruma::serde::Raw::from_json(serde_json::value::to_raw_value(content).map_err(|e| e.to_string())?);
        let failed = client.encryption().encrypt_and_send_raw_to_device(vec![d], typ, raw, strategy).await.map_err(|e| e.to_string())?;
        if failed.is_empty() {
            Ok(())
        } else {
            Err("not delivered (withheld or refused)".into())
        }
    }
    /// The lab gate's raw send (crafted / replayed / foreign messages): no trust check, any user's device.
    pub async fn send_raw(client: &Client, device: &str, typ: &str, content: &Value) -> Result<(), String> {
        let me = client.user_id().ok_or("not signed in")?.to_string();
        PadNode::send_raw_to(client, &me, device, typ, content).await
    }
    pub async fn send_raw_to(client: &Client, user: &str, device: &str, typ: &str, content: &Value) -> Result<(), String> {
        let uid: OwnedUserId = matrix_sdk::ruma::UserId::parse(user).map_err(|e| e.to_string())?;
        let d = client.encryption().get_device(&uid, &OwnedDeviceId::from(device)).await.map_err(|e| e.to_string())?.ok_or_else(|| format!("{device}: not on record"))?;
        Self::encrypt_send(client, &d, typ, content, matrix_sdk_base::crypto::CollectStrategy::AllDevices).await
    }

    /// Queue one message for `device`. Call it under the lock that numbered the message: the device's outbox sends
    /// them one at a time, in that order; a full outbox drops the message (logged).
    fn enqueue(self: &Arc<Self>, device: &str, typ: &'static str, content: Value) {
        if device.is_empty() {
            return;
        }
        let mut ob = self.outbox.lock().unwrap();
        let tx = ob.entry(device.to_string()).or_insert_with(|| self.outbox_worker(device));
        if tx.try_send(Queued { typ, content, at: Instant::now() }).is_err() {
            eprintln!("[pad] the outbox for {device} is full: a {typ} was dropped");
        }
    }
    fn outbox_worker(self: &Arc<Self>, device: &str) -> tokio::sync::mpsc::Sender<Queued> {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Queued>(OUTBOX_MAX);
        let (weak, device) = (Arc::downgrade(self), device.to_string());
        crate::bridge::runtime().spawn(async move {
            loop {
                let item = match tokio::time::timeout(Duration::from_millis(OUTBOX_IDLE_MS), rx.recv()).await {
                    Ok(Some(item)) => item,
                    Ok(None) => return,
                    Err(_) => {
                        // idle: retire — under the map's lock, so nothing can be queued between the check and the removal
                        let Some(me) = weak.upgrade() else { return };
                        let mut ob = me.outbox.lock().unwrap();
                        if rx.is_empty() {
                            ob.remove(&device);
                            return;
                        }
                        continue;
                    }
                };
                let Some(me) = weak.upgrade() else { return };
                let waited = item.at.elapsed().as_millis();
                if waited > SEND_MAX_AGE_MS {
                    me.log(format!("dropped a {} for {device}: it waited too long to go out", item.typ));
                    continue;
                }
                if item.typ == pr::PAD_CMD && !item.is_close() && me.is_cancelled(item.sid()) {
                    continue; // that session was closed / given up here: its close goes, nothing else
                }
                // the whole send fits in the message's budget (queue wait included): never a late command
                let budget = (SEND_MAX_AGE_MS + 2_000).saturating_sub(waited).min(SEND_TIMEOUT_MS as u128).max(1) as u64;
                match tokio::time::timeout(Duration::from_millis(budget), me.send_checked(&device, item.typ, &item.content)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => me.log(format!("not sending {} to {device}: {e}", item.typ)),
                    Err(_) => me.log(format!("sending {} to {device} took too long: given up", item.typ)),
                }
            }
        });
        tx
    }

    fn cancel_sid(&self, sid: &str) {
        let mut c = self.cancelled.lock().unwrap();
        if !c.iter().any(|s| s == sid) {
            c.push_back(sid.to_string());
        }
        while c.len() > 64 {
            c.pop_front();
        }
    }
    fn is_cancelled(&self, sid: &str) -> bool {
        !sid.is_empty() && self.cancelled.lock().unwrap().iter().any(|s| s == sid)
    }

    // ---- receiving ----

    /// One to-device event from the sync loop. Only the two Pad types are read; everything else returns at once.
    pub async fn on_to_device(self: &Arc<Self>, pe_: &ProcessedToDeviceEvent) {
        let Ok(event) = pe_.as_raw().deserialize_as::<Value>() else { return };
        let typ = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if typ != pr::PAD_CMD && typ != pr::PAD_STATE {
            return;
        }
        let content = event.get("content").cloned().unwrap_or(Value::Null);
        if typ == pr::PAD_CMD {
            self.on_cmd(pe_, &event, &content).await;
        } else {
            let facts = self.facts(pe_, &event).await;
            self.on_state(&content, &facts);
        }
    }

    /// The sounds a `sound.play` may use besides the UI's copies: my favourites and the call room's sounds, from the
    /// local store (what sync delivered — no network). Returns the room they belong to, and each sound's name + emoji
    /// (the executor plays it under that name, so the receiver's Recent says "Airhorn", not the url's file name — the
    /// web's receiver names it from the same lists: phase 3).
    /// (favourites, the call room, its sounds, the favourites' names, the room sounds' names)
    #[allow(clippy::type_complexity)]
    async fn store_sounds(&self) -> (Vec<String>, String, Vec<String>, HashMap<String, (String, String)>, HashMap<String, (String, String)>) {
        use matrix_sdk::ruma::events::GlobalAccountDataEventType;
        let mut favs = Vec::new();
        let mut fav_names: HashMap<String, (String, String)> = HashMap::new();
        let mut room_names: HashMap<String, (String, String)> = HashMap::new();
        let name_of = |names: &mut HashMap<String, (String, String)>, c: &Value, u: &str| {
            let n = c.get("name").and_then(|x| x.as_str()).unwrap_or("").trim().chars().take(64).collect::<String>();
            let e = c.get("emoji").and_then(|x| x.as_str()).unwrap_or("").trim().chars().take(16).collect::<String>();
            if !n.is_empty() {
                names.entry(u.to_string()).or_insert((n, e));
            }
        };
        if let Ok(Some(raw)) = self.client.account().account_data_raw(GlobalAccountDataEventType::from("dev.tinhat.soundboard_personal")).await {
            if let Ok(v) = raw.deserialize_as_unchecked::<Value>() {
                for s in v.get("sounds").and_then(|x| x.as_array()).map(|a| a.as_slice()).unwrap_or(&[]) {
                    if let Some(u) = s.get("sourceUrl").and_then(|u| u.as_str()).filter(|u| !u.is_empty()) {
                        favs.push(u.to_string());
                        name_of(&mut fav_names, s, u);
                    }
                }
            }
        }
        let room = self.rx.lock().unwrap().snapshot().call.as_ref().map(|c| c.0.clone()).unwrap_or_default();
        let mut room_sounds = Vec::new();
        if let Some(r) = matrix_sdk::ruma::RoomId::parse(room.as_str()).ok().and_then(|r| self.client.get_room(&r)) {
            for (_, _, content) in crate::appstate::get_state_all(&r, "dev.tinhat.soundboard").await {
                if let Some(u) = content.get("sourceUrl").and_then(|u| u.as_str()).filter(|u| !u.is_empty()) {
                    room_sounds.push(u.to_string());
                    name_of(&mut room_names, &content, u);
                }
            }
        }
        (favs, room, room_sounds, fav_names, room_names)
    }

    async fn on_cmd(self: &Arc<Self>, pe_: &ProcessedToDeviceEvent, event: &Value, content: &Value) {
        let op = clean(content.get("op").and_then(|o| o.as_str()).unwrap_or("?"));
        // the sound lists first (only for a decrypted command of mine), the trust facts LAST: nothing awaits between
        // the facts and the decision
        let mine = matches!(pe_, ProcessedToDeviceEvent::Decrypted { encryption_info, .. } if Some(&*encryption_info.sender) == self.client.user_id());
        let (favs, sounds_room, room_sounds, mut names, room_names) =
            if mine && op == "sound.play" { self.store_sounds().await } else { (Vec::new(), String::new(), Vec::new(), HashMap::new(), HashMap::new()) };
        let facts = self.facts(pe_, event).await;
        let v = {
            let mut rx = self.rx.lock().unwrap();
            // the call room's sounds count only if the call is still that room (a switch meanwhile: none)
            let same_room = rx.snapshot().call.as_ref().map(|c| c.0 == sounds_room).unwrap_or(false);
            let rs: &[String] = if same_room { &room_sounds } else { &[] };
            // names: the favourites', then (only for the call it still is) the room's — a favourite's name wins
            if same_room {
                for (u, ne) in room_names {
                    names.entry(u).or_insert(ne);
                }
            }
            let (v, outs) = rx.on_cmd(content, &facts, &favs, rs, now_ms());
            self.dispatch_named(outs, &names); // under the lock: the states leave in rseq order
            v
        };
        let from = clean(&[&facts.sender_device, &facts.olm_sender, &facts.event_sender].into_iter().find(|s| !s.is_empty()).cloned().unwrap_or_default());
        if !v.ok && !v.reply {
            self.log_rejection(format!("cmd {op} from {from}: REJECTED ({})", v.reason));
        } else if !v.ok {
            self.log(format!("cmd {op} from {from}: refused {}", v.why));
        } else {
            self.log(format!("cmd {op} seq {} from {from}: ok{}", v.re, if v.run { ", running" } else { "" }));
        }
        self.update_chips();
        self.ensure_ticker();
    }

    fn on_state(self: &Arc<Self>, content: &Value, facts: &PadFacts) {
        let now = now_ms();
        if self.with_ctl(|c| ((), c.on_state(content, facts, now))).is_none() {
            self.log_rejection("state dropped: no-session".into());
        }
    }

    /// The receiver's outs: states into their device's outbox (call it under the rx lock), commands to the UI.
    fn dispatch(self: &Arc<Self>, outs: Vec<Out>) {
        self.dispatch_named(outs, &HashMap::new())
    }

    /// dispatch + a `startSound`'s name and emoji from `names` (favourites / the call room's sounds), when known.
    fn dispatch_named(self: &Arc<Self>, outs: Vec<Out>, names: &HashMap<String, (String, String)>) {
        for o in outs {
            match o {
                Out::Send { device, content } => self.enqueue(&device, pr::PAD_STATE, content),
                Out::Exec { token, cmds } => {
                    let cmds = cmds
                        .iter()
                        .map(|c| {
                            let mut j = c.to_json();
                            if let pe::Exec::StartSound { url } = c {
                                if let (Some((n, e)), Some(m)) = (names.get(url), j.as_object_mut()) {
                                    m.insert("name".into(), json!(n));
                                    m.insert("emoji".into(), json!(e));
                                }
                            }
                            j
                        })
                        .collect();
                    self.emit(PadEvent::Exec { token, cmds: Value::Array(cmds) })
                }
            }
        }
    }

    fn update_chips(&self) {
        let now = self.rx.lock().unwrap().chips(now_ms());
        let changed = {
            let mut c = self.chips.lock().unwrap();
            if *c != now {
                *c = now.clone();
                true
            } else {
                false
            }
        };
        if changed {
            self.emit(PadEvent::Chips(now));
        }
    }

    // ---- the receiver's UI side ----

    /// The UI's snapshot of this device's call (`gen` already filled in by the caller).
    pub fn publish_state(self: &Arc<Self>, snap: &Value) {
        let s = Snapshot::parse(snap);
        let busy = {
            let mut rx = self.rx.lock().unwrap();
            let outs = rx.set_snapshot(s, now_ms());
            self.dispatch(outs);
            rx.busy()
        };
        if busy {
            self.ensure_ticker();
        }
    }
    /// The UI is about to run command `token`: may it? (`live` = the call this device is in right now.)
    pub fn claim(self: &Arc<Self>, token: i64, live: Option<(String, String)>) -> bool {
        let mut rx = self.rx.lock().unwrap();
        let (ok, outs) = rx.claim(token, live, now_ms());
        self.dispatch(outs);
        if !ok {
            eprintln!("[pad] command {token} not run: no longer valid here");
        }
        ok
    }
    /// The executor ran command `token`.
    pub fn executed(self: &Arc<Self>, token: i64) {
        let mut rx = self.rx.lock().unwrap();
        let outs = rx.executed(token, now_ms());
        self.dispatch(outs);
    }
    /// The call this device's snapshot says it is in (room, gen) — the lab gate's stand-in for the call layer.
    pub fn snapshot_call(&self) -> Option<(String, String)> {
        self.rx.lock().unwrap().snapshot().call.as_ref().map(|c| (c.0.clone(), c.1.clone()))
    }
    /// × on the chip: end that device's Pad for this call.
    pub fn refuse(self: &Arc<Self>, device: &str) {
        {
            let mut rx = self.rx.lock().unwrap();
            let outs = rx.refuse(device, now_ms());
            self.dispatch(outs);
        }
        self.log(format!("pad from {} closed here for this call", clean(device)));
        self.update_chips();
    }
    /// Settings → Calls → Pad.
    pub fn set_pad_on(self: &Arc<Self>, on: bool) {
        {
            let mut rx = self.rx.lock().unwrap();
            let outs = rx.set_pad_on(on, now_ms());
            self.dispatch(outs);
        }
        self.update_chips();
    }
    pub fn pad_on(&self) -> bool {
        self.rx.lock().unwrap().pad_on()
    }

    // ---- the controller ----

    /// Run `f` on the session this device drives (None = there is none). Its commands go into the target's outbox
    /// INSIDE the same lock that gave them their seq (they leave in seq order); the other events are handled after.
    fn with_ctl<R>(self: &Arc<Self>, f: impl FnOnce(&mut Controller) -> (R, Vec<CEv>)) -> Option<R> {
        let (r, rest, sid) = {
            let mut g = self.ctl.lock().unwrap();
            let c = g.as_mut()?;
            let (r, evs) = f(c);
            let (device, sid) = (c.device.clone(), c.sid().to_string());
            let mut rest = Vec::new();
            for e in evs {
                match e {
                    CEv::Send(content) => self.enqueue(&device, pr::PAD_CMD, content),
                    other => rest.push(other),
                }
            }
            (r, rest, sid)
        };
        self.handle(rest, &sid);
        Some(r)
    }

    /// The controller's events of session `sid` — dropped if that session was replaced meanwhile (an old answer must
    /// not settle a new session's press: seqs restart).
    fn handle(self: &Arc<Self>, evs: Vec<CEv>, sid: &str) {
        let mut remote = false;
        for e in evs {
            let current = self.ctl.lock().unwrap().as_ref().map(|c| c.sid() == sid).unwrap_or(false);
            match e {
                CEv::Send(_) => {} // queued by with_ctl / open_pad
                CEv::State => remote = true,
                CEv::Ack { .. } | CEv::Failed { .. } | CEv::Ended(_) if !current => {}
                CEv::Ack { seq, ok, why, op } => {
                    remote = true;
                    self.emit(PadEvent::Ack { seq, ok, why, op });
                }
                CEv::Failed { seq, op } => {
                    remote = true;
                    self.emit(PadEvent::Failed { seq, op });
                }
                CEv::Ended(why) => {
                    remote = true;
                    if why == "unreachable" {
                        self.cancel_sid(sid); // given up: nothing queued for it goes out late
                    }
                    self.log(format!("pad session ended: {why}"));
                    self.emit(PadEvent::Ended(why));
                }
                CEv::Reopen(lost) => {
                    // only for the session that lost it — checked and replaced in one locked step
                    if self.replace_ctl(Some(&lost), None) {
                        self.log("the receiver lost our session: opened a new one".into());
                        self.emit(PadEvent::Remote);
                        self.ensure_ticker();
                    }
                }
                CEv::Dropped(reason) => self.log_rejection(format!("state dropped: {reason}")),
            }
        }
        if remote {
            self.emit(PadEvent::Remote);
        }
    }

    /// Under one lock: (if `only_if` is given) check the current session is that sid, then replace it with a fresh
    /// one (new sid) for `target` (room, device; default: the current one's) and queue its open; the old session's
    /// queued commands are cancelled. false = nothing done.
    fn replace_ctl(self: &Arc<Self>, only_if: Option<&str>, target: Option<(&str, &str)>) -> bool {
        let mut g = self.ctl.lock().unwrap();
        if let Some(sid) = only_if {
            if g.as_ref().map(|c| c.sid() != sid).unwrap_or(true) {
                return false;
            }
        }
        let (room, device) = match (target, g.as_ref()) {
            (Some((r, d)), _) => (r.to_string(), d.to_string()),
            (None, Some(c)) => (c.room.clone(), c.device.clone()),
            (None, None) => return false,
        };
        if let Some(old) = g.as_ref() {
            self.cancel_sid(old.sid());
        }
        let (c, evs) = Controller::open(&room, &device, &mint_sid(), now_ms());
        for e in evs {
            if let CEv::Send(content) = e {
                self.enqueue(&device, pr::PAD_CMD, content);
            }
        }
        *g = Some(c);
        true
    }

    /// Start driving `device` (another of my devices, in the call in `room`): a new session (new sid), installed and
    /// its open queued in one locked step.
    pub fn open_pad(self: &Arc<Self>, room: &str, device: &str) {
        self.replace_ctl(None, Some((room, device)));
        self.emit(PadEvent::Remote);
        self.ensure_ticker();
    }
    /// Close the session this device drives (the window closed): its close goes out and it is removed, in one step;
    /// anything else still queued for it is not sent.
    pub fn close_pad(self: &Arc<Self>) {
        {
            let mut g = self.ctl.lock().unwrap();
            if let Some(mut c) = g.take() {
                for e in c.close(now_ms()) {
                    if let CEv::Send(content) = e {
                        self.enqueue(&c.device, pr::PAD_CMD, content);
                    }
                }
                self.cancel_sid(c.sid());
            }
        }
        self.emit(PadEvent::Remote);
    }
    /// One command (`{op, …}`); its seq (0 = not sent now).
    pub fn send(self: &Arc<Self>, cmd: &Value) -> i64 {
        let now = now_ms();
        let seq = self.with_ctl(|c| c.send(cmd, now)).unwrap_or(0);
        if seq > 0 {
            self.emit(PadEvent::Remote);
        }
        self.ensure_ticker();
        seq
    }
    pub fn set_visible(self: &Arc<Self>, vis: bool) {
        let now = now_ms();
        self.with_ctl(|c| ((), c.set_visible(vis, now)));
    }
    /// The session this device drives, for the window: `{room, device, sid, reach, opened, ended, gen, state,
    /// pending: [{seq, op, cmd}]}` (null when none).
    pub fn remote_view(&self) -> Value {
        let g = self.ctl.lock().unwrap();
        let Some(c) = g.as_ref() else { return Value::Null };
        let pending: Vec<Value> = c.pending_ops().iter().map(|(s, op)| json!({"seq": s, "op": op, "cmd": c.pending_content(*s).cloned().unwrap_or(Value::Null)})).collect();
        json!({
            "room": c.room, "device": c.device, "sid": c.sid(), "reach": c.reach.as_str(), "opened": c.opened,
            "ended": c.ended, "gen": c.gen, "state": c.state, "ops": c.ops(), "pending": pending,
        })
    }

    // ---- the clock ----

    /// One task ticks both machines every 100 ms while either has something to do, then stops.
    fn ensure_ticker(self: &Arc<Self>) {
        if self.ticking.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = Arc::downgrade(self);
        crate::bridge::runtime().spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let Some(n) = weak.upgrade() else { return };
                let now = now_ms();
                let busy_rx = {
                    let mut rx = n.rx.lock().unwrap();
                    let outs = rx.tick(now);
                    n.dispatch(outs);
                    rx.busy()
                };
                n.update_chips();
                let busy_ctl = n
                    .with_ctl(|c| {
                        let evs = c.tick(now);
                        (c.ended.is_none(), evs)
                    })
                    .unwrap_or(false);
                if !busy_rx && !busy_ctl {
                    n.ticking.store(false, Ordering::SeqCst);
                    // something started between the check and the store: keep going
                    let again = n.rx.lock().unwrap().busy() || n.ctl.lock().unwrap().as_ref().map(|c| c.ended.is_none()).unwrap_or(false);
                    if !again || n.ticking.swap(true, Ordering::SeqCst) {
                        return;
                    }
                }
            }
        });
    }

    // ---- discovery ----

    /// My other devices in the calls of these rooms: `{rooms: {roomId: [{device, name, ready, why, createdTs}]},
    /// names: {device: display name}, verified}` (rooms without one are left out).
    pub async fn targets(&self, rooms: &[matrix_sdk::Room]) -> Value {
        use matrix_sdk::ruma::events::call::member::CallMemberEventContent;
        let (me, my_dev) = self.my_ids();
        let Ok(uid) = matrix_sdk::ruma::UserId::parse(me.as_str()) else { return json!({"rooms": {}, "names": {}}) };
        let verified = self.self_verified().await;
        let mut trusted: Vec<String> = Vec::new();
        let mut names = serde_json::Map::new();
        if let Ok(devs) = self.client.encryption().get_user_devices(&uid).await {
            for d in devs.devices() {
                let id = d.device_id().to_string();
                if d.is_verified_with_cross_signing() && !d.is_deleted() {
                    trusted.push(id.clone());
                }
                names.insert(id.clone(), json!(d.display_name().filter(|n| !n.trim().is_empty()).unwrap_or(id.as_str())));
            }
        }
        let now = now_ms();
        let mut out = serde_json::Map::new();
        for room in rooms {
            let Ok(raws) = room.get_state_events_static::<CallMemberEventContent>().await else { continue };
            let events: Vec<Value> = raws.iter().filter_map(|r| serde_json::to_value(r).ok()).collect();
            let ts = pr::pad_targets(&me, &my_dev, now, verified, &trusted, &events);
            if ts.is_empty() {
                continue;
            }
            let list: Vec<Value> = ts
                .iter()
                .map(|t| {
                    let name = names.get(&t.device).cloned().unwrap_or(json!(t.device));
                    json!({"device": t.device, "name": name, "ready": t.ready, "why": t.why, "createdTs": t.created_ts})
                })
                .collect();
            out.insert(room.room_id().to_string(), Value::Array(list));
        }
        // the room-list bar shows the NEWEST such call (the web's order: features/pad/order.ts)
        let order = crate::pad_engine::calls_newest_first(&out);
        json!({"rooms": out, "order": order, "names": names, "verified": verified})
    }

    /// The call room's sounds (store), for the Pad window's "This room" row: `[{name, url, emoji}]`, newest first.
    pub async fn room_sounds(&self, room_id: &str) -> Value {
        let Some(room) = matrix_sdk::ruma::RoomId::parse(room_id).ok().and_then(|r| self.client.get_room(&r)) else { return json!([]) };
        let mut out: Vec<(i64, Value)> = Vec::new();
        for (_, _, c) in crate::appstate::get_state_all(&room, "dev.tinhat.soundboard").await {
            let url = c.get("sourceUrl").and_then(|u| u.as_str()).unwrap_or("");
            let name = c.get("name").and_then(|u| u.as_str()).unwrap_or("");
            if url.is_empty() || name.trim().is_empty() {
                continue;
            }
            let at = c.get("addedAt").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64;
            out.push((at, json!({"name": name.trim(), "url": url, "emoji": c.get("emoji").and_then(|e| e.as_str()).unwrap_or("")})));
        }
        out.sort_by(|a, b| b.0.cmp(&a.0));
        Value::Array(out.into_iter().map(|x| x.1).collect())
    }
}
