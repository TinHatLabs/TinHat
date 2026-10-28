// Wired into the LiveKit-SDK publish path (next milestone); standalone for now.
#![allow(dead_code)]
//! Element Call media-key (E2EE) exchange over Matrix.
//!
//! TRANSPORT (verified against matrix-js-sdk `develop`, which EC pins, 2026-06-19):
//! per-sender LiveKit media keys travel ONLY over **encrypted to-device** events of
//! type `io.element.call.encryption_keys` (`ToDeviceKeyTransport` is the sole
//! transport on develop — there is no room-event transport). So peers' keys arrive
//! via the sync loop's `response.to_device` (see `call_ctl::ingest_todevice_*`), NOT
//! as room timeline events. Senders pick recipients straight from the call
//! memberships, so our MatrixRTC membership must be recognized (see `rtc.rs`).
//!
//! SEND side: EC peers install our key the SAME way we install theirs — from an ENCRYPTED
//! to-device `io.element.call.encryption_keys` event. So we MUST announce our key over
//! to-device (`send_our_key_to_device`), NOT just the room timeline. (An earlier comment
//! here claimed the room broadcast sufficed because "EC decodes our track purely from the
//! key we set locally in our own FrameCryptor" — that was WRONG: our local key only
//! ENCRYPTS our outgoing media; each PEER needs to RECEIVE our key to decrypt it. With
//! only the room event, EC never gets our key and our audio/video is silent/black to them.
//! The room-event broadcast was RETIRED on 2026-09-21: in an unencrypted room it handed the
//! homeserver (and every later joiner) the raw key, and in an encrypted room each mirror
//! arrived as `m.room.encrypted`, which no push rule can quiet, so long calls chimed every
//! peer every 15 minutes. Receiving legacy room mirrors (`fetch_peer_keys`) still works.)
//!
//! Verified against the live EC bundle (2026-06-18):
//!   content = { keys: [{ index, key }], device_id, call_id: "", sent_ts }
//!   key     = 16 random bytes, base64url, NO padding (raw IKM, not derived)
//!   index   = increments on key rotation
//! Peers map the key to a LiveKit participant by `userId:deviceId` — exactly the
//! identity lk-jwt `/sfu/get` puts in our JWT, so it lines up by construction.
//! The raw 16 bytes are the HKDF input keying material; LiveKit/EC derive the
//! actual AES-128-GCM key via HKDF-SHA256(salt="LKFrameEncryptionKey").
//!
//! KEY INDEX (corrected 2026-07-13; an earlier note here said "0..15, keyringSize
//! = 16" — that was wrong). Element Call's MatrixKeyProvider sets keyringSize 256
//! and rotates `(prev + 1) % 256` (element-call/src/e2ee/matrixKeyProvider.ts;
//! matrix-js-sdk RTCEncryptionManager). We advance our OWN index every session (see
//! [`advance_persisted_key_index`]) so a rejoin never collides with our previous key
//! in a peer's per-(identity,index) tracking — and, since the calls round (M2, rotate on
//! leave: callrules.rs Planner, call_ctl.rs rotation_driver), on every rotation inside a
//! call too: a holder of our key leaves → after 3 s a new key at index+1 goes to the rest,
//! and we encrypt with it 3 s later. Each `keys` object is still sent as its own event
//! (current first, then the pending one). We cap at 255 slots because
//! libwebrtc clamps its ring to MAX_KEYRING_SIZE=255 while bounds-checking against
//! the unclamped option — an index-255 frame would read past the ring.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use matrix_sdk::ruma::events::macros::EventContent;
use matrix_sdk::Room;
use serde::{Deserialize, Serialize};

/// One `{index, key}` entry in an encryption-keys event's `keys` array.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct KeyEntry {
    pub index: u8,
    pub key: String,
}

/// `io.element.call.encryption_keys` as a **to-device** event — the transport EC
/// peers actually use to share their per-sender media keys (encrypted per device).
/// We must receive these to decrypt peers' audio/video (room events only carry our
/// own key on this deployment).
#[derive(Clone, Debug, Deserialize, Serialize, EventContent)]
#[ruma_event(type = "io.element.call.encryption_keys", kind = ToDevice)]
pub struct CallEncryptionKeysEventContent {
    pub keys: Vec<KeyEntry>,
    pub device_id: String,
    #[serde(default)]
    pub call_id: String,
    #[serde(default)]
    pub sent_ts: Option<u64>,
}

/// The Element Call key-distribution event type (room transport).
pub const ENCRYPTION_KEYS_EVENT: &str = "io.element.call.encryption_keys";

/// Usable keyring slots. EC's index space is nominally %256, but libwebrtc clamps the
/// actual ring to 255 slots (and index 255 would read out of bounds — see
/// matrix_key_provider), so OUR indices stay in `0..255`.
pub const KEY_RING_SIZE: u8 = 255;

/// A fresh 16-byte per-sender media key: raw bytes (to feed the LiveKit
/// KeyProvider as HKDF IKM) plus the base64url(no-pad) string EC expects on the wire.
pub fn make_sender_key() -> ([u8; 16], String) {
    use rand::RngCore;
    let mut key = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut key);
    let b64 = URL_SAFE_NO_PAD.encode(key);
    (key, b64)
}

/// The wire (room-event) encoding of a media key — base64url, no padding.
pub fn key_to_b64(key: &[u8; 16]) -> String {
    URL_SAFE_NO_PAD.encode(key)
}

/// Next key index (wraps within the usable keyring, never emits 255).
pub fn next_index(index: u8) -> u8 {
    index.wrapping_add(1) % KEY_RING_SIZE
}

/// The key index to use for a NEW join. ADVANCES on every call — within this process
/// unconditionally, and across sessions via a persisted counter.
///
/// Re-minting at a FIXED index made every rejoin collide with our previous key in
/// every peer's per-(identity,index) tracking: web EC's OutdatedKeyFilter could
/// silently discard the fresh key (its to-device entries are stamped with the
/// receiver's LOCAL clock, room events with origin_server_ts — cross-domain skew
/// decides), and our own room-transport poll could reinstall the historical one.
/// A fresh index has no history anywhere, so the collision class vanishes.
///
/// The IN-PROCESS counter is what guarantees that: an unwritable/unreadable
/// `~/.config` (root-owned dir, read-only mount, full disk) must NOT silently drop us
/// back to a fixed index every join — that would restore the whole bug. Persistence is
/// best-effort on top: it only decides where the NEXT SESSION starts, and when it is
/// unavailable we start from a random slot rather than the historically-crowded 0.
pub fn advance_persisted_key_index() -> u8 {
    use std::sync::atomic::{AtomicU16, Ordering};
    static LAST: AtomicU16 = AtomicU16::new(u16::MAX); // u16::MAX = not seeded yet

    let path = key_index_path();
    let mut next = match LAST.load(Ordering::Relaxed) {
        u16::MAX => {
            // First call this session: continue from the persisted counter if we can
            // read it, else from a random slot (never a fixed one).
            let prev = std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| s.trim().parse::<u8>().ok())
                .map(|v| v % KEY_RING_SIZE);
            match prev {
                Some(p) => next_index(p),
                None => {
                    use rand::Rng;
                    rand::thread_rng().gen_range(0..KEY_RING_SIZE)
                }
            }
        }
        last => next_index(last as u8),
    };
    next %= KEY_RING_SIZE; // never 255 (outside the cryptor's usable ring)
    LAST.store(next as u16, Ordering::Relaxed);

    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, next.to_string()) {
        // Not fatal: LAST above already guarantees this session never reuses an index.
        eprintln!("[e2ee] couldn't persist key index ({e}) — the next SESSION will pick a random start");
    }
    next
}

fn key_index_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    base.join("tinhat/media-key-index")
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// `send_our_key` (the room-event key mirror) was removed on 2026-09-21 — see the module
// header. Media keys travel over encrypted to-device only (`send_our_key_to_device`).

/// Announce our LiveKit media key over ENCRYPTED to-device — the transport the deployed
/// Element Call (`ToDeviceKeyTransport`) actually listens on. Without this, EC peers never
/// install our key and our outgoing audio/video stays silent/black to them (see the module
/// header). Content shape mirrors matrix-js-sdk `ToDeviceKeyTransport.sendKey` byte-for-byte:
/// `keys` is a SINGLE object `{index,key}` (NOT an array), `member.{claimed_device_id,id}`,
/// `room_id`, `session`, `sent_ts`; the key is STANDARD base64 (EC's `encodeBase64`).
/// `our_member_id` is EC's membership id (`userId:deviceId`, colon form). Returns the
/// devices we couldn't reach (empty = all delivered); the caller logs them.
pub async fn send_our_key_to_device(
    client: &matrix_sdk::Client,
    room_id: &matrix_sdk::ruma::RoomId,
    our_device_id: &str,
    our_member_id: &str,
    key: &[u8; 16],
    index: u8,
    recipients: &[matrix_sdk::encryption::identities::Device],
) -> matrix_sdk::Result<Vec<(matrix_sdk::ruma::OwnedUserId, matrix_sdk::ruma::OwnedDeviceId)>> {
    use base64::engine::general_purpose::STANDARD;
    if recipients.is_empty() {
        return Ok(Vec::new());
    }
    let key_b64 = STANDARD.encode(key);
    let content = serde_json::json!({
        "keys": { "index": index, "key": key_b64 }, // SINGLE object, not an array
        "room_id": room_id.as_str(),
        "member": { "claimed_device_id": our_device_id, "id": our_member_id },
        "session": { "call_id": "", "application": "m.call", "scope": "m.room" },
        "sent_ts": now_ms(),
    });
    let raw: matrix_sdk::ruma::serde::Raw<matrix_sdk::ruma::events::AnyToDeviceEventContent> =
        matrix_sdk::ruma::serde::Raw::from_json(
            serde_json::value::to_raw_value(&content).expect("serialize encryption_keys content"),
        );
    let devices: Vec<&matrix_sdk::encryption::identities::Device> = recipients.iter().collect();
    client
        .encryption()
        .encrypt_and_send_raw_to_device(
            devices,
            ENCRYPTION_KEYS_EVENT,
            raw,
            matrix_sdk_base::crypto::CollectStrategy::AllDevices,
        )
        .await
}

/// A peer's media key parsed from an `io.element.call.encryption_keys` event.
#[derive(Clone, Debug)]
pub struct ReceivedKey {
    /// LiveKit participant identity (`userId:deviceId`) this key decrypts.
    pub identity: String,
    pub key: [u8; 16],
    pub index: u8,
}

/// Decode an EC media key string to its 16 raw bytes. We BROADCAST keys as
/// base64url-nopad, but EC's to-device transport encodes with STANDARD base64
/// (matrix-js-sdk `encodeBase64`), so try every common variant — picking the wrong
/// one silently fails to decode and the key never installs (black tiles + silence).
pub fn decode_key(b64: &str) -> Option<[u8; 16]> {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE};
    let bytes = URL_SAFE_NO_PAD
        .decode(b64)
        .ok()
        .or_else(|| STANDARD.decode(b64).ok())
        .or_else(|| URL_SAFE.decode(b64).ok())
        .or_else(|| STANDARD_NO_PAD.decode(b64).ok())?;
    bytes.try_into().ok()
}

/// Parse peer media keys from EITHER transport's content shape:
///  * to-device (current EC): `{ keys: {index,key}, member: {claimed_device_id, id}, room_id, session, sent_ts }`
///  * room event (legacy):    `{ keys: [{index,key}], device_id, call_id, sent_ts }`
/// `sender` is the event's Matrix sender. The LiveKit identity is `member.id` when
/// present (EC sets it to `userId:deviceId`), else `{sender}:{device_id}`.
pub fn parse_encryption_keys_any(sender: &str, content: &serde_json::Value) -> Vec<ReceivedKey> {
    // Tolerate being handed the whole event or just the inner content.
    let content = content.get("content").unwrap_or(content);
    let member = content.get("member");
    let device_id = member
        .and_then(|m| m.get("claimed_device_id"))
        .and_then(|d| d.as_str())
        .or_else(|| content.get("device_id").and_then(|d| d.as_str()))
        .unwrap_or("");
    if device_id.is_empty() {
        eprintln!("[e2ee] parse: no device_id (member.claimed_device_id / device_id missing)");
        return Vec::new();
    }
    // The key MUST be installed under the SAME identity LiveKit uses for the peer's
    // track. Our logs show colon form (@user:server:DEVICE); EC's member.id /
    // membership state key may use underscore form — install under ALL candidates so
    // whatever lk-jwt actually emits matches (extra keys under unused ids are harmless).
    let colon = format!("{sender}:{device_id}");
    let underscore = format!("_{sender}_{device_id}");
    let mut identities: Vec<String> = vec![colon.clone()];
    let member_id = member
        .and_then(|m| m.get("id"))
        .and_then(|i| i.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        // Legacy session memberships default memberId to `userId:deviceId`.
        .unwrap_or_else(|| colon.clone());
    // `member.id` is chosen by the SENDER. It is only ever installed as a LiveKit
    // identity when it is one of the forms derived from the authenticated `sender`
    // (`userId:deviceId` / `_userId_deviceId`). A free-form member.id let any sender
    // install a key under ANY participant's identity — including ours, replacing the
    // key our own media is encrypted with (Astra review 2026-09-21, finding 1). The
    // raw value is still fed to the hashed-identity candidate below: that hash is
    // bound to `sender` too, so it can never collide with someone else's identity.
    if member_id != colon && member_id != underscore {
        eprintln!(
            "[e2ee] parse: member.id {member_id:?} is not derived from sender {sender} — not installing under it"
        );
    }
    if !identities.iter().any(|x| x == &underscore) {
        identities.push(underscore);
    }
    // Sticky/MSC4143 (RTC-kind) peers connect to LiveKit under a HASHED identity:
    // encodeUnpaddedBase64(sha256(JSON([userId,deviceId,memberId]))) (matrix-js-sdk
    // RTCEncryptionManager / computeRtcIdentityRaw). Legacy peers use userId:deviceId
    // (which we already cover). Installing under this extra candidate is inert if the
    // SFU doesn't use it, and fixes decrypt if it does — so add it defensively.
    if let Some(h) = hashed_identity(sender, device_id, &member_id) {
        if !identities.iter().any(|x| x == &h) {
            identities.push(h);
        }
    }

    // Collect (index, key) entries — `keys` may be a single object OR an array, and
    // `index` may be a JSON number OR a numeric string ("0").
    let mut raw: Vec<(u64, String)> = Vec::new();
    let mut take = |entry: &serde_json::Value| {
        match (
            entry.get("index").and_then(as_index),
            entry.get("key").and_then(|s| s.as_str()),
        ) {
            (Some(i), Some(s)) => raw.push((i, s.to_string())),
            (i, s) => eprintln!(
                "[e2ee] parse: keys entry missing index/key (index={i:?} key_present={})",
                s.is_some()
            ),
        }
    };
    match content.get("keys") {
        Some(k) if k.is_object() => take(k),
        Some(k) if k.is_array() => k.as_array().unwrap().iter().for_each(|e| take(e)),
        other => eprintln!(
            "[e2ee] parse: no usable 'keys' field (got {})",
            other.map(|v| v.to_string()).unwrap_or_else(|| "absent".into())
        ),
    }

    let mut out = Vec::new();
    for (i, s) in raw {
        match decode_key(&s) {
            Some(key) => {
                for id in &identities {
                    out.push(ReceivedKey { identity: id.clone(), key, index: i as u8 });
                }
            }
            None => eprintln!("[e2ee] parse: decode_key failed (len {})", s.len()),
        }
    }
    out
}

/// Accept an index that is a JSON number OR a numeric string ("0").
fn as_index(v: &serde_json::Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
}

/// A sticky/MSC4143 peer's LiveKit identity, matching matrix-js-sdk's
/// `computeRtcIdentityRaw` = `encodeUnpaddedBase64(sha256(JSON.stringify([userId,
/// deviceId, memberId])))`. `serde_json` serializes a 3-string array exactly like
/// JS `JSON.stringify` (no spaces), and matrix-js-sdk's `encodeUnpaddedBase64` uses
/// the STANDARD base64 alphabet without padding.
pub(crate) fn hashed_identity(user_id: &str, device_id: &str, member_id: &str) -> Option<String> {
    use base64::engine::general_purpose::STANDARD_NO_PAD;
    use sha2::{Digest, Sha256};
    let json = serde_json::to_string(&[user_id, device_id, member_id]).ok()?;
    let digest = Sha256::digest(json.as_bytes());
    Some(STANDARD_NO_PAD.encode(digest))
}

/// Parse the `keys` array out of an `io.element.call.encryption_keys` event sent
/// by `sender`. Returns one [`ReceivedKey`] per advertised index. (For receiving
/// peers' streams; not needed for publish-only, but keeps us a well-behaved member.)
pub fn parse_keys_event(sender: &str, content: &serde_json::Value) -> Vec<ReceivedKey> {
    // NB: do NOT reject on call_id. We send call_id="" (EC requires that of us), but
    // peers may advertise a non-empty call_id for the room call — rejecting those
    // dropped every peer key, so we could never decrypt anyone (black + silent).
    let device_id = match content.get("device_id").and_then(|d| d.as_str()) {
        Some(d) if !d.is_empty() => d,
        _ => return Vec::new(),
    };
    let identity = format!("{sender}:{device_id}");
    let Some(keys) = content.get("keys").and_then(|k| k.as_array()) else {
        return Vec::new();
    };
    keys.iter()
        .filter_map(|k| {
            let index = k.get("index").and_then(|i| i.as_u64())? as u8;
            let key = decode_key(k.get("key").and_then(|s| s.as_str())?)?;
            Some(ReceivedKey {
                identity: identity.clone(),
                key,
                index,
            })
        })
        .collect()
}
