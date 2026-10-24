//! key_intake.rs — the native's media-key intake. Every Olm-decrypted `io.element.call.encryption_keys`
//! to-device event goes through the trust rules ([`crate::keyrules`], the same rules as Tin Hat Web's
//! keys.ts `intake`) before anything reaches the streamer's keyring:
//!   * the Olm sender key is bound to the sender's device RECORD (the web's `bindDevice`): the device the SDK
//!     names must be on record with the Curve25519 key the message was encrypted with (an embedded, self-signed
//!     `sender_device_keys` naming another device of the same user is refused), or the device is found by that
//!     key. Not on record yet = pending, and a key query is started in the background;
//!   * rules 1, 2, 3 and 5 next (Olm sender = event sender, claimed device = the bound device, our call's room,
//!     not ourselves), then the sender's room membership and the room's live, well-formed call sessions (rule 4);
//!   * a key that is valid but whose device / membership isn't known yet is held (≤ [`keyrules::PENDING_MAX`],
//!     ≤ [`keyrules::PENDING_TTL_MS`]) and re-checked, never installed on trust;
//!   * just before installing, the call (room + join generation) and the rules are checked again (the web's
//!     re-check after its awaits);
//!   * accepted: ≤ 4 entries per event, indices 0..=254, ≤ 8 indices held per device (oldest evicted from the
//!     registry), installed under the colon, underscore and hashed identity (member.id bound to the device).
//! Nothing here waits on the network: this runs inline in the sync loop. Member lists and device keys that
//! are missing are fetched by one background task per room / user, and the held key is re-checked.
//! Log lines: `[recv-key-td] REJECTED: …` / `holding: …` / `key #N from @u:dev for !room` (as the web), and the
//! per-identity `[recv2-parse]` trace for accepted keys.

use crate::keyrules::{self, OlmInfo, Verdict};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

struct Held {
    enc: OlmInfo,
    sender: String,
    event: serde_json::Value,
    at: Instant,
    /// the call (join generation) it was held for: a held key never crosses into another call
    gen: u64,
    /// arrival order: a retried copy never replaces a newer one
    seq: u64,
}

fn pending() -> &'static Mutex<Vec<Held>> {
    static P: OnceLock<Mutex<Vec<Held>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(Vec::new()))
}
static SEQ: AtomicU64 = AtomicU64::new(0);

/// Per-device book-keeping for one call: (sender, device) → indices held (oldest first) and the hashed identity
/// in use. Reset when the call (room or join generation) changes.
struct Owners {
    call: Option<(String, u64)>,
    indices: HashMap<(String, String), Vec<u8>>,
    hashed: HashMap<(String, String), String>,
}

fn owners() -> &'static Mutex<Owners> {
    static O: OnceLock<Mutex<Owners>> = OnceLock::new();
    O.get_or_init(|| Mutex::new(Owners { call: None, indices: HashMap::new(), hashed: HashMap::new() }))
}

static RECHECK_RUNNING: AtomicBool = AtomicBool::new(false);

/// Background fetches (a room's member list, a user's device keys): one in flight per key, the same key at most
/// every 10 s, at most [`MAX_FETCHES`] keys remembered (older entries pruned) — no sender can make it grow or fan out.
struct Fetches {
    started: HashMap<String, Instant>,
    in_flight: HashSet<String>,
}
const MAX_FETCHES: usize = 64;
fn fetches() -> &'static Mutex<Fetches> {
    static F: OnceLock<Mutex<Fetches>> = OnceLock::new();
    F.get_or_init(|| Mutex::new(Fetches { started: HashMap::new(), in_flight: HashSet::new() }))
}
fn may_fetch(key: &str) -> bool {
    let mut f = fetches().lock().unwrap();
    if f.in_flight.contains(key) || f.started.get(key).map(|t| t.elapsed().as_secs() < 10).unwrap_or(false) {
        return false;
    }
    f.started.retain(|_, t| t.elapsed().as_secs() < 60);
    if f.started.len() >= MAX_FETCHES || f.in_flight.len() >= MAX_FETCHES {
        return false;
    }
    f.started.insert(key.to_string(), Instant::now());
    f.in_flight.insert(key.to_string());
    true
}
/// A fetch finished: free its slot and re-check the held keys now (their device / membership may be known).
fn fetch_done(key: &str) {
    fetches().lock().unwrap().in_flight.remove(key);
    recheck_now().notify_one();
}
fn recheck_now() -> &'static tokio::sync::Notify {
    static N: OnceLock<tokio::sync::Notify> = OnceLock::new();
    N.get_or_init(tokio::sync::Notify::new)
}

fn event_content(event: &serde_json::Value) -> &serde_json::Value {
    event.get("content").unwrap_or(&serde_json::Value::Null)
}

/// One Olm-decrypted key event from the sync loop (`enc` = the SDK's EncryptionInfo).
pub async fn ingest(enc: OlmInfo, event: serde_json::Value) {
    let sender = event.get("sender").and_then(|s| s.as_str()).unwrap_or("").to_string();
    let call = crate::call_ctl::key_intake_room();
    let gen = call.as_ref().map(|c| c.1).unwrap_or(0);
    intake(enc, sender, event, Instant::now(), gen, SEQ.fetch_add(1, Ordering::SeqCst)).await;
}

/// The membership facts for the rules: the sender's room membership (None = not known yet) and the call room's
/// live sessions. Local store only; a member list that isn't loaded is fetched in the background.
async fn facts(call_room: &str, sender: &str) -> (Option<String>, Vec<keyrules::LiveSession>) {
    let Some(client) = crate::session::client() else { return (None, Vec::new()) };
    let (Ok(rid), Ok(uid)) = (matrix_sdk::ruma::RoomId::parse(call_room), matrix_sdk::ruma::UserId::parse(sender)) else {
        return (None, Vec::new());
    };
    let Some(room) = client.get_room(&rid) else { return (None, Vec::new()) };
    // the call sessions first, the room membership LAST (closest to the decision: a leave/kick that lands meanwhile
    // is seen by the re-check)
    let sessions = crate::rtc::live_sessions(&room).await;
    let member = room.get_member_no_sync(&uid).await.ok().flatten();
    let fk = format!("members|{call_room}");
    if member.is_none() && !room.are_members_synced() && may_fetch(&fk) {
        let r = room.clone();
        crate::bridge::runtime().spawn(async move {
            if let Err(e) = r.sync_members().await {
                eprintln!("[recv-key-td] member list of {} not loaded: {e}", r.room_id());
            }
            fetch_done(&fk);
        });
    }
    let membership = member.map(|m| m.membership().as_str().to_string());
    (membership, sessions)
}

/// The web's `bindDevice`: bind the Olm sender key to a device RECORD of the sender. Ok(enc) with
/// `sender_device` = the bound device, or None when it isn't on record (yet); Err(reason) when the record
/// contradicts the message.
async fn bind_device(mut enc: OlmInfo) -> Result<OlmInfo, String> {
    let Some(client) = crate::session::client() else { return Ok(OlmInfo { sender_device: None, ..enc }) };
    let Ok(uid) = matrix_sdk::ruma::UserId::parse(&enc.sender) else { return Ok(OlmInfo { sender_device: None, ..enc }) };
    let curve = enc.sender_curve.clone().unwrap_or_default();
    let query = |client: &matrix_sdk::Client| {
        let fk = format!("keys|{}", enc.sender);
        if may_fetch(&fk) {
            let c = client.clone();
            let u = uid.clone();
            crate::bridge::runtime().spawn(async move {
                let _ = c.encryption().request_user_identity(&u).await; // a /keys/query for the user
                fetch_done(&fk);
            });
        }
    };
    if let Some(dev) = enc.sender_device.clone() {
        let did = matrix_sdk::ruma::OwnedDeviceId::from(dev.as_str());
        match client.encryption().get_device(&uid, &did).await.ok().flatten() {
            Some(d) => {
                let rec = d.curve25519_key().map(|k| k.to_base64()).unwrap_or_default();
                if !curve.is_empty() && !rec.is_empty() && rec != curve {
                    return Err(format!("{}/{dev}: Curve25519 key does not match the device record", enc.sender));
                }
                Ok(enc)
            }
            None => {
                query(&client);
                enc.sender_device = None;
                Ok(enc)
            }
        }
    } else {
        if !curve.is_empty() {
            if let Ok(devs) = client.encryption().get_user_devices(&uid).await {
                if let Some(d) = devs.devices().find(|d| d.curve25519_key().map(|k| k.to_base64()).as_deref() == Some(curve.as_str())) {
                    enc.sender_device = Some(d.device_id().to_string());
                    return Ok(enc);
                }
            }
        }
        query(&client);
        Ok(enc)
    }
}

async fn verdict_now(enc: &OlmInfo, event_sender: &str, content: &serde_json::Value, call_room: &str) -> (Verdict, Vec<keyrules::LiveSession>) {
    let client = crate::session::client();
    let own_user = client.as_ref().and_then(|c| c.user_id().map(|u| u.to_string())).unwrap_or_default();
    let own_dev = client.as_ref().and_then(|c| c.device_id().map(|d| d.to_string())).unwrap_or_default();
    // Every rule that needs no lookup first (membership assumed) — the rules are ordered, so a reject or hold
    // here is the final verdict, and an attacker's event costs no store scan.
    let assumed: HashSet<(String, String)> = [(enc.sender.clone(), enc.sender_device.clone().unwrap_or_default())].into_iter().collect();
    let assume_join = |_: &str| Some("join".to_string());
    let v1 = keyrules::check_key_event(
        Some(enc),
        event_sender,
        content,
        &keyrules::IntakeCtx { call_room_id: call_room, own_user_id: &own_user, own_device_id: &own_dev, live_members: &assumed, membership_of: &assume_join },
    );
    if !v1.is_ok() {
        return (v1, Vec::new());
    }
    let (membership, sessions) = facts(call_room, &enc.sender).await;
    let live = keyrules::live_key_members(&sessions);
    let who = enc.sender.clone();
    let ms = move |u: &str| if u == who { membership.clone() } else { None };
    let v = keyrules::check_key_event(
        Some(enc),
        event_sender,
        content,
        &keyrules::IntakeCtx { call_room_id: call_room, own_user_id: &own_user, own_device_id: &own_dev, live_members: &live, membership_of: &ms },
    );
    (v, sessions)
}

async fn intake(enc: OlmInfo, event_sender: String, event: serde_json::Value, first_at: Instant, gen: u64, seq: u64) {
    let call = crate::call_ctl::key_intake_room();
    if let Some((_, g)) = &call {
        if *g != gen {
            return; // held for another call (a leave / rejoin happened): never crosses over
        }
    }
    let call_room = call.as_ref().map(|c| c.0.clone()).unwrap_or_default();
    let content = event_content(&event).clone();
    // No call, or a key for another room: refused before any lookup (the web: no sink → dropped at once), so an
    // event that can never be installed starts no device query and holds no pending slot.
    let room_id = content.get("room_id").and_then(|r| r.as_str()).unwrap_or("");
    if call_room.is_empty() || room_id != call_room {
        eprintln!(
            "[recv-key-td] REJECTED: key for {}, not our call ({})",
            if room_id.is_empty() { "?" } else { room_id },
            if call_room.is_empty() { "none" } else { call_room.as_str() }
        );
        return;
    }
    let enc = match bind_device(enc).await {
        Ok(e) => e,
        Err(reason) => {
            eprintln!("[recv-key-td] REJECTED: {reason}");
            return;
        }
    };
    let (mut verdict, mut sessions) = verdict_now(&enc, &event_sender, &content, &call_room).await;
    if verdict.is_ok() {
        // The web's re-check after its awaits: same call, and the device is STILL a joined, live call member.
        if crate::call_ctl::key_intake_room() != call {
            verdict = Verdict::Reject("the call changed while the key was checked".into());
        } else {
            let (v2, s2) = verdict_now(&enc, &event_sender, &content, &call_room).await;
            verdict = if crate::call_ctl::key_intake_room() != call { Verdict::Reject("the call changed while the key was checked".into()) } else { v2 };
            sessions = s2;
        }
    }
    match verdict {
        Verdict::Ok { sender, device, room_id } => {
            let mid = sessions
                .iter()
                .find(|s| s.sender == sender && s.device_id == device && s.well_formed)
                .map(|s| s.membership_id.clone())
                .unwrap_or_default();
            install(&sender, &device, &room_id, gen, &content, &mid);
        }
        Verdict::Pending(reason) => {
            eprintln!("[recv-key-td] holding: {reason}");
            hold(Held { enc, sender: event_sender, event, at: first_at, gen, seq });
        }
        Verdict::Reject(reason) => eprintln!("[recv-key-td] REJECTED: {reason}"),
    }
}

/// Install an accepted key under the device's identities, keeping at most MAX_INDICES_PER_DEVICE per device.
fn install(sender: &str, device: &str, room_id: &str, gen: u64, content: &serde_json::Value, live_membership_id: &str) {
    let colon = format!("{sender}:{device}");
    let underscore = format!("_{sender}_{device}");
    let member_in = content.get("member").and_then(|m| m.get("id")).and_then(|i| i.as_str());
    let member_id = keyrules::bound_member_id(member_in, sender, device, live_membership_id);
    if let Some(m) = member_in {
        if !m.is_empty() && m != member_id {
            eprintln!("[recv-key-td] member.id {m} is neither derived from {colon} nor its membershipID — using {colon}");
        }
    }
    let hashed = crate::e2ee_keys::hashed_identity(sender, device, &member_id);
    let owner = (sender.to_string(), device.to_string());
    let mut o = owners().lock().unwrap();
    let this_call = Some((room_id.to_string(), gen));
    if o.call != this_call {
        o.call = this_call;
        o.indices.clear();
        o.hashed.clear();
    }
    let mut identities = vec![colon.clone(), underscore];
    if let Some(h) = hashed {
        // One hashed (membershipID-bound) identity per device: a changed membershipID retires the old one.
        if let Some(prev) = o.hashed.insert(owner.clone(), h.clone()) {
            if prev != h {
                for &i in o.indices.get(&owner).map(|v| v.as_slice()).unwrap_or(&[]) {
                    tinhat_streamer::call::forget_peer_key(&prev, i);
                }
            }
        }
        identities.push(h);
    }
    for e in keyrules::key_entries(content) {
        let (Some(index), Some(key)) = (
            e.get("index").and_then(keyrules::valid_index),
            e.get("key").and_then(|k| k.as_str()).and_then(crate::e2ee_keys::decode_key),
        ) else {
            eprintln!("[recv-key-td] {colon}: entry without a usable index/key");
            continue;
        };
        let held = o.indices.entry(owner.clone()).or_default();
        for old in keyrules::note_index(held, index) {
            for id in &identities {
                tinhat_streamer::call::forget_peer_key(id, old);
            }
            eprintln!("[recv-key-td] {colon}: more than {} indices — dropped the oldest ({old})", keyrules::MAX_INDICES_PER_DEVICE);
        }
        for id in &identities {
            eprintln!("[recv2-parse] (sync) sender={sender} -> identity={id} index={index} keybytes_len=16");
            if !tinhat_streamer::call::install_peer_key(id, index, key) {
                eprintln!("[recv-key-td] {id}#{index}: the keyring refused it");
            }
        }
        eprintln!("[recv-key-td] key #{index} from {colon} for {room_id}");
    }
}

/// One entry per (sender, claimed device, indices): a newer copy replaces the older one — but a RETRIED copy
/// (older) never replaces a newer one that arrived while it was being re-checked.
fn hold(h: Held) {
    {
        let mut p = pending().lock().unwrap();
        let id = |x: &Held| {
            let c = event_content(&x.event);
            let idx: Vec<String> = keyrules::key_entries(c).iter().map(|e| e.get("index").map(|i| i.to_string()).unwrap_or_default()).collect();
            format!("{}|{}|{}", x.sender, keyrules::claimed_device(c), idx.join(","))
        };
        let me = id(&h);
        if p.iter().any(|x| id(x) == me && x.seq > h.seq) {
            return; // a newer copy is already held
        }
        p.retain(|x| id(x) != me);
        p.push(h);
        while p.len() > keyrules::PENDING_MAX {
            p.remove(0);
        }
    }
    if !RECHECK_RUNNING.swap(true, Ordering::SeqCst) {
        crate::bridge::runtime().spawn(recheck_loop());
    }
}

/// Re-run held keys through the rules (their device / membership may have synced meanwhile) until none are left.
async fn recheck_loop() {
    let mut first = true;
    loop {
        let wait = std::time::Duration::from_millis(if first { 1_500 } else { 2_000 });
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = recheck_now().notified() => {}
        }
        first = false;
        let batch: Vec<Held> = std::mem::take(&mut *pending().lock().unwrap());
        for h in batch {
            if h.at.elapsed().as_millis() as u64 > keyrules::PENDING_TTL_MS {
                eprintln!(
                    "[recv-key-td] {}/{}: still no live call membership after {}s — key dropped",
                    h.sender,
                    keyrules::claimed_device(event_content(&h.event)),
                    keyrules::PENDING_TTL_MS / 1000
                );
                continue;
            }
            // (re-held with its own arrival seq: a copy that arrived meanwhile stays the newer one)
            intake(h.enc, h.sender, h.event, h.at, h.gen, h.seq).await;
        }
        if pending().lock().unwrap().is_empty() {
            RECHECK_RUNNING.store(false, Ordering::SeqCst);
            // a key held between the check and the store above starts its own loop
            if pending().lock().unwrap().is_empty() || RECHECK_RUNNING.swap(true, Ordering::SeqCst) {
                return;
            }
        }
    }
}
