//! recorder_keys.rs — the Memories recorder keeps CALL keys only (2026-11-14).
//!
//! The recorder account is a joined member of every room that records, so every app shares that room's megolm keys
//! with it (`m.room_key`), and it could also collect them as forwarded keys, from a key backup or from a key bundle.
//! Those are keys to people's CHAT. The recorder needs none of them: a call's media keys arrive as Olm-encrypted
//! `io.element.call.encryption_keys` to-device events (key_intake.rs), and everything else it reads (the record
//! marker, call memberships, opt-outs) is room state, which is never encrypted.
//!
//! Inside matrix-sdk, a decrypted `m.room_key` goes straight from the Olm layer to the crypto store, before the app
//! sees the sync response, and the Olm envelope hides the event type until then. So the filter sits at the store: in
//! recorder mode the client's crypto store is wrapped in [`CallKeysOnly`], which
//!   * never WRITES an inbound megolm session (m.room_key, m.forwarded_room_key, a backup import, our own outgoing
//!     session's copy), our own outgoing megolm session, a received room-key bundle, a room-key request or a request
//!     for the backup key, or the key-backup decryption key (as a field or as a received secret);
//!   * never READS one back either: what an older recorder already stored stays on disk (deleting it is the
//!     operator's call, see the purge) but this process can't use it: no lookup, list, count, backup, bundle, request
//!     or backup-key read returns it. A forwarded key therefore never counts as "requested".
//! Each refused write logs one line, "dropped a chat key (recorder keeps call keys only)", with what it was and its
//! room, never any key material. Olm sessions, devices, identities, our account and the sync token are kept: that's
//! the call-key channel. The client is also built with the backup download set to Manual.
//!
//! [`purge_on_start`]: an OPT-IN (`TINHAT_VOD_PURGE_CHAT_KEYS=1`) delete of every chat key already in the recorder's
//! store, before the client opens it. Default OFF: deleting is the operator's call.
//!
//! The GUI app never uses any of this ([`set_active`] is called by `vodsvc::run` only).

use matrix_sdk::cross_process_lock::CrossProcessLockGeneration;
use matrix_sdk::ruma::{events::secret::request::SecretName, DeviceId, OwnedDeviceId, RoomId, TransactionId, UserId};
use matrix_sdk_crypto::olm::{
    InboundGroupSession, OlmMessageHash, OutboundGroupSession, PrivateCrossSigningIdentity, SenderDataType, Session,
};
use matrix_sdk_crypto::store::types::{
    BackupKeys, Changes, DehydratedDeviceKey, PendingChanges, RoomKeyCounts, RoomKeyWithheldEntry,
    RoomPendingKeyBundleDetails, RoomSettings, StoredRoomKeyBundleData, TrackedUser,
};
use matrix_sdk_crypto::store::{CryptoStore, CryptoStoreError};
use matrix_sdk_crypto::vodozemac::Curve25519PublicKey;
use matrix_sdk_crypto::{Account, DeviceData, GossipRequest, SecretInfo, UserIdentityData};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use zeroize::Zeroizing;

type Result<T, E = CryptoStoreError> = std::result::Result<T, E>;

/// The one log line per dropped key (no key material: what it was and its room only).
pub const DROP_LINE: &str = "dropped a chat key (recorder keeps call keys only)";
/// The opt-in purge switch.
pub const PURGE_ENV: &str = "TINHAT_VOD_PURGE_CHAT_KEYS";
/// matrix-sdk-sqlite's crypto database file name, in the store directory.
pub const CRYPTO_DB: &str = "matrix-sdk-crypto.sqlite3";
/// The secret name of the key-backup decryption key.
const BACKUP_SECRET: &str = "m.megolm_backup.v1";
/// The key-backup decryption key's row in the sqlite store's `kv` table (matrix-sdk-sqlite 0.19), which custom values
/// share.
const BACKUP_KEY_KV: &str = "recovery_key_v1";

static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Recorder mode: every client built from now on in this process keeps call keys only. Called once, by
/// `vodsvc::run`, before the recorder's client is restored or signed in.
pub fn set_active() {
    ACTIVE.store(true, Ordering::SeqCst);
}

pub fn active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}

/// A gossip request the recorder must neither store nor honour: a room-key request, or a request for the backup key.
fn chat_key_request(info: &SecretInfo) -> Option<String> {
    match info {
        SecretInfo::KeyRequest(k) => Some(k.room_id().to_string()),
        SecretInfo::SecretRequest(name) if name.as_str() == BACKUP_SECRET => Some("-".to_string()),
        SecretInfo::SecretRequest(_) => None,
    }
}

/// The log line for one refused key: what it was and its room, nothing else.
fn drop_line(what: &str, room: &str) -> String {
    format!("[vod] {DROP_LINE}: {what} for {room}")
}

/// What one save would have written that the recorder refuses: (what, room).
fn strip(changes: &mut Changes) -> Vec<(&'static str, String)> {
    let mut dropped = Vec::new();
    for s in changes.inbound_group_sessions.drain(..) {
        dropped.push(("room key", s.room_id().to_string()));
    }
    for s in changes.outbound_group_sessions.drain(..) {
        dropped.push(("own outgoing room key", s.room_id().to_string()));
    }
    for b in changes.received_room_key_bundles.drain(..) {
        dropped.push(("room key bundle", b.bundle_data.room_id.to_string()));
    }
    changes.key_requests.retain(|r| match chat_key_request(&r.info) {
        Some(room) => {
            dropped.push((if room == "-" { "key backup key request" } else { "room key request" }, room));
            false
        }
        None => true,
    });
    if changes.backup_decryption_key.take().is_some() {
        dropped.push(("key backup key", "-".to_string()));
    }
    changes.secrets.retain(|s| {
        let backup = s.secret_name.as_str() == BACKUP_SECRET;
        if backup {
            dropped.push(("key backup key", "-".to_string()));
        }
        !backup
    });
    dropped
}

/// The recorder's crypto store: the real store underneath, minus every chat key (see the module docs).
#[derive(Debug)]
pub struct CallKeysOnly<S> {
    inner: S,
    dropped: AtomicUsize,
}

impl<S> CallKeysOnly<S> {
    pub fn new(inner: S) -> Self {
        Self { inner, dropped: AtomicUsize::new(0) }
    }

    /// How many chat keys this store has refused to write so far.
    pub fn dropped(&self) -> usize {
        self.dropped.load(Ordering::SeqCst)
    }

    fn note(&self, what: &str, room: &str) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
        eprintln!("{}", drop_line(what, room));
    }
}

#[matrix_sdk::async_trait]
impl<S: CryptoStore> CryptoStore for CallKeysOnly<S> {
    type Error = CryptoStoreError;

    // ---- writes: chat keys are dropped, the rest goes through ----

    async fn save_changes(&self, mut changes: Changes) -> Result<()> {
        for (what, room) in strip(&mut changes) {
            self.note(what, &room);
        }
        self.inner.save_changes(changes).await.map_err(Into::into)
    }

    async fn save_inbound_group_sessions(
        &self,
        sessions: Vec<InboundGroupSession>,
        _backed_up_to_version: Option<&str>,
    ) -> Result<()> {
        for s in sessions {
            self.note("room key", s.room_id().as_str());
        }
        Ok(())
    }

    // ---- reads of chat keys: nothing (what an older recorder stored stays unusable) ----

    async fn get_inbound_group_session(&self, _room_id: &RoomId, _session_id: &str) -> Result<Option<InboundGroupSession>> {
        Ok(None)
    }

    async fn get_inbound_group_sessions(&self) -> Result<Vec<InboundGroupSession>> {
        Ok(Vec::new())
    }

    async fn inbound_group_session_counts(&self, _backup_version: Option<&str>) -> Result<RoomKeyCounts> {
        Ok(RoomKeyCounts::default())
    }

    async fn get_inbound_group_sessions_by_room_id(&self, _room_id: &RoomId) -> Result<Vec<InboundGroupSession>> {
        Ok(Vec::new())
    }

    async fn get_inbound_group_sessions_for_device_batch(
        &self,
        _curve_key: Curve25519PublicKey,
        _sender_data_type: SenderDataType,
        _after_session_id: Option<String>,
        _limit: usize,
    ) -> Result<Vec<InboundGroupSession>> {
        Ok(Vec::new())
    }

    async fn inbound_group_sessions_for_backup(&self, _backup_version: &str, _limit: usize) -> Result<Vec<InboundGroupSession>> {
        Ok(Vec::new())
    }

    async fn get_outbound_group_session(&self, _room_id: &RoomId) -> Result<Option<OutboundGroupSession>> {
        Ok(None)
    }

    async fn load_backup_keys(&self) -> Result<BackupKeys> {
        // not even loaded: the inner store would decrypt an old backup key into memory to hand it over
        Ok(BackupKeys { decryption_key: None, backup_version: None })
    }

    async fn get_received_room_key_bundle_data(
        &self,
        _room_id: &RoomId,
        _user_id: &UserId,
    ) -> Result<Option<StoredRoomKeyBundleData>> {
        Ok(None)
    }

    async fn get_secrets_from_inbox(&self, secret_name: &SecretName) -> Result<Vec<Zeroizing<String>>> {
        if secret_name.as_str() == BACKUP_SECRET {
            return Ok(Vec::new());
        }
        self.inner.get_secrets_from_inbox(secret_name).await.map_err(Into::into)
    }

    async fn get_outgoing_secret_requests(&self, request_id: &TransactionId) -> Result<Option<GossipRequest>> {
        let r = self.inner.get_outgoing_secret_requests(request_id).await.map_err(Into::into)?;
        Ok(r.filter(|r| chat_key_request(&r.info).is_none()))
    }

    async fn get_secret_request_by_info(&self, secret_info: &SecretInfo) -> Result<Option<GossipRequest>> {
        if chat_key_request(secret_info).is_some() {
            return Ok(None);
        }
        self.inner.get_secret_request_by_info(secret_info).await.map_err(Into::into)
    }

    async fn get_unsent_secret_requests(&self) -> Result<Vec<GossipRequest>> {
        let mut v = self.inner.get_unsent_secret_requests().await.map_err(Into::into)?;
        v.retain(|r| chat_key_request(&r.info).is_none());
        Ok(v)
    }

    // ---- everything else: a plain pass-through ----

    async fn load_account(&self) -> Result<Option<Account>> {
        self.inner.load_account().await.map_err(Into::into)
    }

    async fn load_identity(&self) -> Result<Option<PrivateCrossSigningIdentity>> {
        self.inner.load_identity().await.map_err(Into::into)
    }

    async fn save_pending_changes(&self, changes: PendingChanges) -> Result<()> {
        self.inner.save_pending_changes(changes).await.map_err(Into::into)
    }

    async fn get_sessions(&self, sender_key: &str) -> Result<Option<Vec<Session>>> {
        self.inner.get_sessions(sender_key).await.map_err(Into::into)
    }

    async fn get_withheld_info(&self, room_id: &RoomId, session_id: &str) -> Result<Option<RoomKeyWithheldEntry>> {
        self.inner.get_withheld_info(room_id, session_id).await.map_err(Into::into)
    }

    async fn get_withheld_sessions_by_room_id(&self, room_id: &RoomId) -> Result<Vec<RoomKeyWithheldEntry>> {
        self.inner.get_withheld_sessions_by_room_id(room_id).await.map_err(Into::into)
    }

    async fn mark_inbound_group_sessions_as_backed_up(
        &self,
        backup_version: &str,
        room_and_session_ids: &[(&RoomId, &str)],
    ) -> Result<()> {
        self.inner.mark_inbound_group_sessions_as_backed_up(backup_version, room_and_session_ids).await.map_err(Into::into)
    }

    async fn reset_backup_state(&self) -> Result<()> {
        self.inner.reset_backup_state().await.map_err(Into::into)
    }

    async fn load_dehydrated_device_pickle_key(&self) -> Result<Option<DehydratedDeviceKey>> {
        self.inner.load_dehydrated_device_pickle_key().await.map_err(Into::into)
    }

    async fn delete_dehydrated_device_pickle_key(&self) -> Result<()> {
        self.inner.delete_dehydrated_device_pickle_key().await.map_err(Into::into)
    }

    async fn load_tracked_users(&self) -> Result<Vec<TrackedUser>> {
        self.inner.load_tracked_users().await.map_err(Into::into)
    }

    async fn save_tracked_users(&self, users: &[(&UserId, bool)]) -> Result<()> {
        self.inner.save_tracked_users(users).await.map_err(Into::into)
    }

    async fn get_device(&self, user_id: &UserId, device_id: &DeviceId) -> Result<Option<DeviceData>> {
        self.inner.get_device(user_id, device_id).await.map_err(Into::into)
    }

    async fn get_user_devices(&self, user_id: &UserId) -> Result<HashMap<OwnedDeviceId, DeviceData>> {
        self.inner.get_user_devices(user_id).await.map_err(Into::into)
    }

    async fn get_own_device(&self) -> Result<DeviceData> {
        self.inner.get_own_device().await.map_err(Into::into)
    }

    async fn get_user_identity(&self, user_id: &UserId) -> Result<Option<UserIdentityData>> {
        self.inner.get_user_identity(user_id).await.map_err(Into::into)
    }

    async fn is_message_known(&self, message_hash: &OlmMessageHash) -> Result<bool> {
        self.inner.is_message_known(message_hash).await.map_err(Into::into)
    }

    async fn delete_outgoing_secret_requests(&self, request_id: &TransactionId) -> Result<()> {
        self.inner.delete_outgoing_secret_requests(request_id).await.map_err(Into::into)
    }

    async fn delete_secrets_from_inbox(&self, secret_name: &SecretName) -> Result<()> {
        self.inner.delete_secrets_from_inbox(secret_name).await.map_err(Into::into)
    }

    async fn get_room_settings(&self, room_id: &RoomId) -> Result<Option<RoomSettings>> {
        self.inner.get_room_settings(room_id).await.map_err(Into::into)
    }

    async fn get_pending_key_bundle_details_for_room(&self, room_id: &RoomId) -> Result<Option<RoomPendingKeyBundleDetails>> {
        self.inner.get_pending_key_bundle_details_for_room(room_id).await.map_err(Into::into)
    }

    async fn get_all_rooms_pending_key_bundles(&self) -> Result<Vec<RoomPendingKeyBundleDetails>> {
        self.inner.get_all_rooms_pending_key_bundles().await.map_err(Into::into)
    }

    async fn has_downloaded_all_room_keys(&self, room_id: &RoomId) -> Result<bool> {
        self.inner.has_downloaded_all_room_keys(room_id).await.map_err(Into::into)
    }

    // custom values share the sqlite store's kv table with the backup key's row: that one name is refused
    async fn get_custom_value(&self, key: &str) -> Result<Option<Vec<u8>>> {
        if key == BACKUP_KEY_KV {
            return Ok(None);
        }
        self.inner.get_custom_value(key).await.map_err(Into::into)
    }

    async fn set_custom_value(&self, key: &str, value: Vec<u8>) -> Result<()> {
        if key == BACKUP_KEY_KV {
            self.note("key backup key", "-");
            return Ok(());
        }
        self.inner.set_custom_value(key, value).await.map_err(Into::into)
    }

    async fn remove_custom_value(&self, key: &str) -> Result<()> {
        self.inner.remove_custom_value(key).await.map_err(Into::into)
    }

    async fn try_take_leased_lock(
        &self,
        lease_duration_ms: u32,
        key: &str,
        holder: &str,
    ) -> Result<Option<CrossProcessLockGeneration>> {
        self.inner.try_take_leased_lock(lease_duration_ms, key, holder).await.map_err(Into::into)
    }

    async fn next_batch_token(&self) -> Result<Option<String>> {
        self.inner.next_batch_token().await.map_err(Into::into)
    }

    async fn close(&self) -> Result<()> {
        self.inner.close().await.map_err(Into::into)
    }

    async fn reopen(&self) -> Result<()> {
        self.inner.reopen().await.map_err(Into::into)
    }

    async fn get_size(&self) -> Result<Option<usize>> {
        self.inner.get_size().await.map_err(Into::into)
    }
}

/// The recorder's client builder: the same sqlite stores, in the same place, as `sqlite_store(dir, pass)` (so an
/// existing recorder session restores unchanged; the same cross-process lock holder name as matrix-sdk's default),
/// with the crypto store wrapped in [`CallKeysOnly`], and the key backup never downloaded.
pub async fn builder(
    b: matrix_sdk::ClientBuilder,
    dir: &Path,
    pass: Option<&str>,
) -> std::result::Result<matrix_sdk::ClientBuilder, String> {
    use matrix_sdk::config::StoreConfig;
    use matrix_sdk::cross_process_lock::CrossProcessLockConfig;
    use matrix_sdk::encryption::{BackupDownloadStrategy, EncryptionSettings};
    use matrix_sdk::{SqliteCryptoStore, SqliteEventCacheStore, SqliteMediaStore, SqliteStateStore, SqliteStoreConfig};
    let cfg = SqliteStoreConfig::new(dir).passphrase(pass);
    let e = |what: &str, err: &dyn std::fmt::Display| format!("recorder store ({what}): {err}");
    let state = SqliteStateStore::open_with_config(&cfg).await.map_err(|x| e("state", &x))?;
    let cache = SqliteEventCacheStore::open_with_config(&cfg).await.map_err(|x| e("event cache", &x))?;
    let media = SqliteMediaStore::open_with_config(&cfg).await.map_err(|x| e("media", &x))?;
    let crypto = SqliteCryptoStore::open_with_config(&cfg).await.map_err(|x| e("crypto", &x))?;
    let stores = StoreConfig::new(CrossProcessLockConfig::multi_process("main"))
        .state_store(state)
        .event_cache_store(cache)
        .media_store(media)
        .crypto_store(CallKeysOnly::new(crypto));
    eprintln!("[vod] recorder store: call keys only (chat keys are dropped, never stored or read)");
    Ok(b.store_config(stores).with_encryption_settings(EncryptionSettings {
        auto_enable_cross_signing: false,
        backup_download_strategy: BackupDownloadStrategy::Manual,
        auto_enable_backups: false,
    }))
}

/// Is the opt-in purge switched on? Exactly "1" (anything else, unset included, = off).
pub fn purge_wanted() -> bool {
    std::env::var(PURGE_ENV).map(|v| v.trim() == "1").unwrap_or(false)
}

/// The crypto store's tables that hold chat keys or what leads to them: inbound + our own outbound megolm sessions,
/// received key bundles, gossip requests (room-key requests and secret requests), received secrets (the backup key
/// arrives there), and the pre-migration secrets table. The recorder needs none of them.
const PURGE_TABLES: [&str; 6] =
    ["inbound_group_session", "outbound_group_session", "received_room_key_bundle", "key_requests", "secrets_inbox", "secrets"];

/// Rows of chat keys in a crypto store database: (inbound group sessions, everything else of [`PURGE_TABLES`] + the
/// backup key row). A table that doesn't exist counts 0.
pub fn chat_key_counts(db: &Path) -> std::result::Result<(u64, u64), String> {
    use rusqlite::{Connection, OpenFlags};
    let c = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|e| e.to_string())?;
    counts(&c)
}

fn counts(c: &rusqlite::Connection) -> std::result::Result<(u64, u64), String> {
    let sessions = count(c, "inbound_group_session")?;
    let mut other = 0;
    for t in PURGE_TABLES.iter().skip(1) {
        other += count(c, t)?;
    }
    if has_table(c, "kv")? {
        other += c
            .query_row("SELECT COUNT(*) FROM kv WHERE key = ?1", [BACKUP_KEY_KV], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())? as u64;
    }
    Ok((sessions, other))
}

fn has_table(c: &rusqlite::Connection, table: &str) -> std::result::Result<bool, String> {
    c.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1", [table], |r| r.get::<_, i64>(0))
        .map(|n| n > 0)
        .map_err(|e| e.to_string())
}

fn count(c: &rusqlite::Connection, table: &str) -> std::result::Result<u64, String> {
    if !has_table(c, table)? {
        return Ok(0);
    }
    c.query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |r| r.get::<_, i64>(0))
        .map(|n| n as u64)
        .map_err(|e| e.to_string())
}

/// What a purge did: the counts (inbound sessions, other chat-key rows) before and after, and whether the clean-up
/// after the delete (WAL checkpoint, VACUUM: the freed pages' old contents) finished. `cleanup` = None when it did.
#[derive(Debug, PartialEq, Eq)]
pub struct Purged {
    pub before: (u64, u64),
    pub after: (u64, u64),
    pub cleanup: Option<String>,
}

/// One `PRAGMA wal_checkpoint(TRUNCATE)`; Err when SQLite reports it blocked (a reader or writer is still on the file).
fn checkpoint(c: &rusqlite::Connection) -> std::result::Result<(), String> {
    let busy: i64 = c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    if busy != 0 {
        return Err("the WAL checkpoint was blocked (another process has the store open)".into());
    }
    Ok(())
}

/// Delete every chat key from the crypto store in `store_dir` (see [`PURGE_TABLES`], plus the backup key), in one
/// EXCLUSIVE transaction (a store that another process holds makes it fail before anything is deleted), with SQLite's
/// secure_delete on, then checkpoint the WAL away and VACUUM. Must run before the client opens the store. Err = nothing
/// was deleted. No store yet = nothing to do.
pub fn purge(store_dir: &Path) -> std::result::Result<Purged, String> {
    use rusqlite::{Connection, TransactionBehavior};
    let db = store_dir.join(CRYPTO_DB);
    if !db.exists() {
        return Ok(Purged { before: (0, 0), after: (0, 0), cleanup: None });
    }
    let mut c = Connection::open(&db).map_err(|e| e.to_string())?;
    c.busy_timeout(std::time::Duration::from_secs(5)).map_err(|e| e.to_string())?;
    // EXCLUSIVE locking mode: in WAL mode a BEGIN EXCLUSIVE alone still lets readers in; this connection keeps the
    // file to itself from its first access until it closes (delete, checkpoint and VACUUM included), so a store that
    // another process has open is refused before anything is deleted.
    c.execute_batch("PRAGMA secure_delete = ON; PRAGMA locking_mode = EXCLUSIVE;").map_err(|e| e.to_string())?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Exclusive).map_err(|e| e.to_string())?;
    let before = counts(&tx)?;
    for table in PURGE_TABLES {
        if has_table(&tx, table)? {
            tx.execute(&format!("DELETE FROM \"{table}\""), []).map_err(|e| e.to_string())?;
        }
    }
    if has_table(&tx, "kv")? {
        tx.execute("DELETE FROM kv WHERE key = ?1", [BACKUP_KEY_KV]).map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    // committed: from here on a failure is reported as an unfinished clean-up, never as "nothing deleted"
    let cleanup = checkpoint(&c)
        .and_then(|_| c.execute_batch("VACUUM;").map_err(|e| e.to_string()))
        .and_then(|_| checkpoint(&c))
        .err();
    let after = counts(&c)?;
    Ok(Purged { before, after, cleanup })
}

/// The recorder's start-up step: if (and only if) the operator switched the purge on, delete the chat keys already
/// in the store and say how many; otherwise say the purge is off and how to switch it on (counts only, no keys). With
/// the purge off (or failed), the keys stay on disk but [`CallKeysOnly`] never reads them.
pub fn purge_on_start(store_dir: &Path) {
    if !purge_wanted() {
        let db = store_dir.join(CRYPTO_DB);
        if !db.exists() {
            eprintln!("[vod] chat-key purge is off (no store yet)");
            return;
        }
        match chat_key_counts(&db) {
            Ok((n, other)) => eprintln!(
                "[vod] chat-key purge is off: {n} chat key(s) and {other} other chat-key row(s) in the store, never read (set {PURGE_ENV}=1 to delete them)"
            ),
            Err(e) => eprintln!("[vod] chat-key purge is off (the store's counts are not readable: {e})"),
        }
        return;
    }
    match purge(store_dir) {
        Ok(p) => {
            eprintln!(
                "[vod] {PURGE_ENV}=1: deleted {} chat key(s) and {} other chat-key row(s) from the recorder's store; left: {} and {}",
                p.before.0, p.before.1, p.after.0, p.after.1
            );
            if let Some(e) = p.cleanup {
                eprintln!("[vod] {PURGE_ENV}=1: the rows are deleted, but the clean-up of the file did not finish ({e}); it runs again on the next start");
            }
        }
        Err(e) => eprintln!("[vod] {PURGE_ENV}=1: the purge failed before deleting anything ({e}); the keys stay unread"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use matrix_sdk::ruma::api::client::keys::{claim_keys, get_keys, upload_keys};
    use matrix_sdk::ruma::serde::Raw;
    use matrix_sdk::ruma::{device_id, room_id, user_id, OneTimeKeyAlgorithm, UInt};
    use matrix_sdk_crypto::olm::{EncryptionSettings, SenderData};
    use matrix_sdk_crypto::store::MemoryStore;
    use matrix_sdk_crypto::types::requests::AnyOutgoingRequest;
    use matrix_sdk_crypto::{CollectStrategy, DecryptionSettings, EncryptionSyncChanges, OlmMachine, OlmMachineBuilder, TrustRequirement};
    use std::collections::BTreeMap;

    const MEDIA_KEYS: &str = "io.element.call.encryption_keys";

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("tinhat-recorder-keys-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
    }

    async fn pair(room: &RoomId) -> (OutboundGroupSession, InboundGroupSession) {
        let a = Account::with_device_id(user_id!("@sender:lab.test"), device_id!("SENDERDEV"));
        a.create_group_session_pair(room, EncryptionSettings::default(), SenderData::unknown()).await.unwrap()
    }

    async fn inbound(room: &RoomId) -> InboundGroupSession {
        pair(room).await.1
    }

    fn settings() -> DecryptionSettings {
        DecryptionSettings { sender_device_trust_requirement: TrustRequirement::Untrusted }
    }

    async fn recorder_machine(dir: &Path, gated: bool) -> OlmMachine {
        use matrix_sdk::SqliteCryptoStore;
        let (u, d) = (user_id!("@recorder:lab.test"), device_id!("RECORDERDEV"));
        let sqlite = SqliteCryptoStore::open(dir, None).await.unwrap();
        let b = OlmMachineBuilder::new(u, d);
        if gated { b.with_crypto_store(CallKeysOnly::new(sqlite)) } else { b.with_crypto_store(sqlite) }.build().await.unwrap()
    }

    /// The recorder machine's keys go up; returns its one-time keys and its signed device keys.
    async fn upload(m: &OlmMachine) -> (BTreeMap<matrix_sdk::ruma::OwnedOneTimeKeyId, Raw<matrix_sdk::ruma::encryption::OneTimeKey>>, Raw<matrix_sdk::ruma::encryption::DeviceKeys>) {
        for r in m.outgoing_requests().await.unwrap() {
            if let AnyOutgoingRequest::KeysUpload(u) = r.request() {
                let otks = u.one_time_keys.clone();
                let dk = u.device_keys.clone().expect("device keys in the first upload");
                let counts = BTreeMap::from([(OneTimeKeyAlgorithm::SignedCurve25519, UInt::new(otks.len() as u64).unwrap())]);
                m.mark_request_as_sent(r.request_id(), &upload_keys::v3::Response::new(counts)).await.unwrap();
                return (otks, dk);
            }
        }
        panic!("no keys upload request");
    }

    fn to_device(sender: &str, ty: &str, content: serde_json::Value) -> Raw<matrix_sdk::ruma::events::AnyToDeviceEvent> {
        let ev = serde_json::json!({ "sender": sender, "type": ty, "content": content });
        Raw::from_json(serde_json::value::to_raw_value(&ev).unwrap())
    }

    async fn sync(m: &OlmMachine, events: Vec<Raw<matrix_sdk::ruma::events::AnyToDeviceEvent>>) -> Vec<(String, bool)> {
        let lists = Default::default();
        let counts = BTreeMap::new();
        let (processed, _) = m
            .receive_sync_changes(
                EncryptionSyncChanges {
                    to_device_events: events,
                    changed_devices: &lists,
                    one_time_keys_counts: &counts,
                    unused_fallback_keys: None,
                    next_batch_token: None,
                },
                &settings(),
            )
            .await
            .unwrap();
        processed
            .iter()
            .map(|p| {
                let ty = p.as_raw().get_field::<String>("type").ok().flatten().unwrap_or_default();
                (ty, matches!(p, matrix_sdk::deserialized_responses::ProcessedToDeviceEvent::Decrypted { .. }))
            })
            .collect()
    }

    /// A call's media key from the sender's device to the recorder's, Olm-encrypted.
    async fn media_key(alice: &OlmMachine, n: u8) -> Raw<matrix_sdk::ruma::events::AnyToDeviceEvent> {
        let dev = alice.get_device(user_id!("@recorder:lab.test"), device_id!("RECORDERDEV"), None).await.unwrap().unwrap();
        let content = serde_json::json!({ "keys": [{ "index": n, "key": "AAAAAAAAAAAAAAAAAAAAAA" }], "room_id": "!call:lab.test" });
        let enc = dev.encrypt_event_raw(MEDIA_KEYS, &content, CollectStrategy::AllDevices).await.unwrap();
        to_device("@alice:lab.test", "m.room.encrypted", enc.deserialize_as::<serde_json::Value>().unwrap())
    }

    struct Run {
        /// the recorder decrypted the planted m.room_key (the test reached the real path)
        saw_room_key: bool,
        /// the call's media key in the SAME sync was decrypted (Olm kept working)
        media_key_same_sync: bool,
        /// the recorder could decrypt the chat message the room key is for
        chat_readable: bool,
        /// after a restart on the same store, a new media key over the same Olm session still decrypts
        media_key_after_restart: bool,
        /// rows of inbound megolm sessions in the recorder's sqlite store afterwards
        rows: u64,
    }

    /// A sender's real `m.room_key` (and, in the same sync, a call media key) to the recorder's device over Olm,
    /// through the recorder machine's sync processing; then a chat message for that key; then a restart on the same
    /// store and another media key.
    async fn planted_room_key(dir: &Path, gated: bool) -> Run {
        let rec_user = user_id!("@recorder:lab.test");
        let rec_dev = device_id!("RECORDERDEV");
        let room = room_id!("!chat:lab.test");
        let rec = recorder_machine(dir, gated).await;
        let (otks, device_keys) = upload(&rec).await;

        let alice = OlmMachine::new(user_id!("@alice:lab.test"), device_id!("ALICEDEV")).await;
        alice.update_tracked_users([rec_user]).await.unwrap();
        for r in alice.outgoing_requests().await.unwrap() {
            if let AnyOutgoingRequest::KeysQuery(_) = r.request() {
                let mut resp = get_keys::v3::Response::new();
                resp.device_keys = BTreeMap::from([(rec_user.to_owned(), BTreeMap::from([(rec_dev.to_owned(), device_keys.clone())]))]);
                alice.mark_request_as_sent(r.request_id(), &resp).await.unwrap();
            }
        }
        let (claim_id, _) = alice.get_missing_sessions([rec_user].into_iter()).await.unwrap().expect("a session to claim");
        let (kid, otk) = otks.into_iter().next().unwrap();
        let claim = claim_keys::v3::Response::new(BTreeMap::from([(
            rec_user.to_owned(),
            BTreeMap::from([(rec_dev.to_owned(), BTreeMap::from([(kid, otk)]))]),
        )]));
        alice.mark_request_as_sent(&claim_id, &claim).await.unwrap();

        let mut events = Vec::new();
        for req in alice.share_room_key(room, [rec_user].into_iter(), EncryptionSettings::default()).await.unwrap() {
            for devices in req.messages.values() {
                for content in devices.values() {
                    events.push(to_device("@alice:lab.test", &req.event_type.to_string(), content.deserialize_as::<serde_json::Value>().unwrap()));
                }
            }
        }
        assert!(!events.is_empty(), "the sender shared its room key with the recorder's device");
        events.push(media_key(&alice, 0).await);
        let seen = sync(&rec, events).await;
        let saw_room_key = seen.iter().any(|(t, d)| t == "m.room_key" && *d);
        let media_key_same_sync = seen.iter().any(|(t, d)| t == MEDIA_KEYS && *d);

        let msg: Raw<matrix_sdk::ruma::events::AnyMessageLikeEventContent> =
            Raw::from_json(serde_json::value::to_raw_value(&serde_json::json!({ "msgtype": "m.text", "body": "a chat line" })).unwrap());
        let enc = alice.encrypt_room_event_raw(room, "m.room.message", &msg).await.unwrap();
        let ev = serde_json::json!({
            "type": "m.room.encrypted", "sender": "@alice:lab.test", "event_id": "$chat1", "origin_server_ts": 1,
            "room_id": room.as_str(), "content": enc.content.deserialize_as::<serde_json::Value>().unwrap(),
        });
        let chat_readable =
            rec.decrypt_room_event(&Raw::from_json(serde_json::value::to_raw_value(&ev).unwrap()), room, &settings()).await.is_ok();

        drop(rec);
        let rec2 = recorder_machine(dir, gated).await;
        let after = sync(&rec2, vec![media_key(&alice, 1).await]).await;
        let media_key_after_restart = after.iter().any(|(t, d)| t == MEDIA_KEYS && *d);
        drop(rec2);
        let rows = chat_key_counts(&dir.join(CRYPTO_DB)).unwrap().0;
        Run { saw_room_key, media_key_same_sync, chat_readable, media_key_after_restart, rows }
    }

    /// FAILING-FIRST: a real m.room_key reaches the recorder (decrypted), its store still has 0 chat keys and the chat
    /// stays unreadable, while the call media key in the same sync, and one after a restart, decrypt. The same flow
    /// on the plain store (the behaviour before this fix) stores the key and reads the chat: the checks can fail.
    #[test]
    fn a_planted_room_key_is_refused_and_not_stored() {
        let rt = rt();
        let dir = tmpdir("plant");
        let g = rt.block_on(planted_room_key(&dir.join("gated"), true));
        eprintln!(
            "recorder store after a planted m.room_key: {} inbound session(s); chat readable: {}; media key same sync: {}, after restart: {}",
            g.rows, g.chat_readable, g.media_key_same_sync, g.media_key_after_restart
        );
        assert!(g.saw_room_key, "the recorder decrypted the planted m.room_key (the test reached the real path)");
        assert_eq!(g.rows, 0, "no chat key in the recorder's store");
        assert!(!g.chat_readable, "the recorder can't read the chat");
        assert!(g.media_key_same_sync, "the call's media key in the same sync still decrypts");
        assert!(g.media_key_after_restart, "after a restart the Olm session (the call-key channel) still works");
        let p = rt.block_on(planted_room_key(&dir.join("plain"), false));
        eprintln!("control (plain store, the old behaviour): {} inbound session(s); chat readable: {}", p.rows, p.chat_readable);
        assert!(p.saw_room_key && p.media_key_same_sync && p.media_key_after_restart);
        assert_eq!(p.rows, 1, "control: without the gate the key IS stored");
        assert!(p.chat_readable, "control: without the gate the chat IS readable");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every way a chat key reaches the store is refused (counted on the store UNDERNEATH), and the rest survives.
    #[test]
    fn every_chat_key_path_is_refused_and_the_rest_is_kept() {
        rt().block_on(async {
            use matrix_sdk_crypto::store::types::BackupDecryptionKey;
            let room = room_id!("!chat:lab.test");
            let gate = CallKeysOnly::new(MemoryStore::new());
            let (out, inb) = pair(room).await;
            let mut changes = Changes::default();
            // m.room_key / m.forwarded_room_key / our own outgoing session and its inbound copy
            changes.inbound_group_sessions = vec![inb, inbound(room).await];
            changes.outbound_group_sessions = vec![out];
            changes.backup_decryption_key = Some(BackupDecryptionKey::from_bytes(&[7u8; 32]));
            changes.backup_version = Some("3".into()); // the version alone is no key: kept
            changes.next_batch_token = Some("s1".into());
            gate.save_changes(changes).await.unwrap();
            // a backup import / an exported-keys import
            gate.save_inbound_group_sessions(vec![inbound(room).await], Some("1")).await.unwrap();
            assert_eq!(gate.inner.inbound_group_session_counts(None).await.unwrap().total, 0, "nothing reached the store");
            assert!(gate.inner.get_outbound_group_session(room).await.unwrap().is_none());
            let bk = gate.inner.load_backup_keys().await.unwrap();
            assert!(bk.decryption_key.is_none(), "the backup key never reached the store");
            assert_eq!(bk.backup_version.as_deref(), Some("3"));
            // the backup key's kv name through the custom-value door
            gate.set_custom_value(BACKUP_KEY_KV, vec![1, 2, 3]).await.unwrap();
            assert!(gate.inner.get_custom_value(BACKUP_KEY_KV).await.unwrap().is_none());
            gate.set_custom_value("other", vec![4]).await.unwrap();
            assert_eq!(gate.get_custom_value("other").await.unwrap(), Some(vec![4]), "other custom values pass");
            assert_eq!(gate.dropped(), 6);
            assert_eq!(gate.next_batch_token().await.unwrap().as_deref(), Some("s1"));
        });
    }

    /// What an OLDER recorder stored (written straight into the store underneath) can't be read through the gate,
    /// and stays on disk (nothing is deleted without the purge).
    #[test]
    fn chat_keys_stored_before_are_never_read() {
        rt().block_on(async {
            use matrix_sdk_crypto::store::types::BackupDecryptionKey;
            let room = room_id!("!chat:lab.test");
            let old = MemoryStore::new();
            let (out, inb) = pair(room).await;
            let sid = inb.session_id().to_string();
            let mut c = Changes::default();
            c.inbound_group_sessions = vec![inb, inbound(room).await];
            c.outbound_group_sessions = vec![out];
            c.backup_decryption_key = Some(BackupDecryptionKey::from_bytes(&[7u8; 32]));
            old.save_changes(c).await.unwrap();
            let gate = CallKeysOnly::new(old);
            assert!(gate.get_inbound_group_session(room, &sid).await.unwrap().is_none());
            assert!(gate.get_inbound_group_sessions().await.unwrap().is_empty());
            assert!(gate.get_inbound_group_sessions_by_room_id(room).await.unwrap().is_empty());
            assert!(gate.inbound_group_sessions_for_backup("1", 100).await.unwrap().is_empty());
            assert_eq!(gate.inbound_group_session_counts(None).await.unwrap().total, 0);
            assert!(gate.get_outbound_group_session(room).await.unwrap().is_none());
            assert!(gate.load_backup_keys().await.unwrap().decryption_key.is_none());
            assert!(gate.inner.load_backup_keys().await.unwrap().decryption_key.is_some(), "the old backup key is still there");
            assert_eq!(gate.inner.inbound_group_session_counts(None).await.unwrap().total, 2, "still on disk, not deleted");
        });
    }

    #[test]
    fn room_key_and_backup_key_requests_are_neither_stored_nor_honoured() {
        use matrix_sdk_crypto::store::types::SecretsInboxItem;
        use matrix_sdk_crypto::types::events::room_key_request::{MegolmV1AesSha2Content, SupportedKeyInfo};
        let room = room_id!("!chat:lab.test");
        let req = |info| GossipRequest {
            request_recipient: user_id!("@recorder:lab.test").to_owned(),
            request_id: TransactionId::new(),
            info,
            sent_out: false,
        };
        let key_req = || {
            req(SecretInfo::KeyRequest(SupportedKeyInfo::MegolmV1AesSha2(MegolmV1AesSha2Content {
                room_id: room.to_owned(),
                sender_key: None,
                session_id: "s".into(),
            })))
        };
        let cross = || req(SecretInfo::SecretRequest(SecretName::from("m.cross_signing.self_signing")));
        let backup = || req(SecretInfo::SecretRequest(SecretName::from(BACKUP_SECRET)));
        let mut c = Changes::default();
        c.key_requests = vec![key_req(), cross(), backup()];
        c.secrets = vec![
            SecretsInboxItem { secret_name: SecretName::from(BACKUP_SECRET), secret: Zeroizing::new("x".into()) },
            SecretsInboxItem { secret_name: SecretName::from("m.cross_signing.user_signing"), secret: Zeroizing::new("y".into()) },
        ];
        let dropped = strip(&mut c);
        let kinds: Vec<&str> = dropped.iter().map(|d| d.0).collect();
        assert_eq!(kinds, vec!["room key request", "key backup key request", "key backup key"]);
        assert_eq!(dropped[0].1, room.as_str());
        assert_eq!(c.key_requests.len(), 1);
        assert!(matches!(&c.key_requests[0].info, SecretInfo::SecretRequest(n) if n.as_str() == "m.cross_signing.self_signing"));
        assert_eq!(c.secrets.len(), 1);
        assert_eq!(c.secrets[0].secret_name.as_str(), "m.cross_signing.user_signing");

        // requests an older recorder stored: a forwarded key never finds "its" request
        rt().block_on(async {
            let old = MemoryStore::new();
            let (k, x, b) = (key_req(), cross(), backup());
            let k_info = k.info.clone();
            let (k_id, x_id) = (k.request_id.clone(), x.request_id.clone());
            let mut c = Changes::default();
            c.key_requests = vec![k, x, b];
            c.secrets = vec![SecretsInboxItem { secret_name: SecretName::from(BACKUP_SECRET), secret: Zeroizing::new("x".into()) }];
            old.save_changes(c).await.unwrap();
            let gate = CallKeysOnly::new(old);
            assert!(gate.get_secret_request_by_info(&k_info).await.unwrap().is_none());
            assert!(gate.get_outgoing_secret_requests(&k_id).await.unwrap().is_none());
            assert!(gate.get_outgoing_secret_requests(&x_id).await.unwrap().is_some(), "a cross-signing request is kept");
            let unsent = gate.get_unsent_secret_requests().await.unwrap();
            assert_eq!(unsent.len(), 1);
            assert!(gate.get_secrets_from_inbox(&SecretName::from(BACKUP_SECRET)).await.unwrap().is_empty());
        });
    }

    /// The drop line names what and where, never the key: checked against a real session's exported key.
    #[test]
    fn the_drop_line_carries_no_key_material() {
        rt().block_on(async {
            let room = room_id!("!chat:lab.test");
            let s = inbound(room).await;
            let exported = serde_json::to_string(&s.export().await).unwrap();
            let key = serde_json::from_str::<serde_json::Value>(&exported).unwrap()["session_key"].as_str().unwrap().to_string();
            assert!(key.len() > 40, "the exported session key");
            let mut c = Changes::default();
            c.inbound_group_sessions = vec![s];
            for (what, r) in strip(&mut c) {
                let line = drop_line(what, &r);
                assert_eq!(line, "[vod] dropped a chat key (recorder keeps call keys only): room key for !chat:lab.test");
                assert!(!line.contains(&key) && !line.contains(&key[..16]));
            }
        });
    }

    /// The opt-in purge, on a passphrase-protected lab store this test makes itself: every chat key (inbound and
    /// outbound sessions, a room-key request, the backup key) is gone; the account and the Olm channel survive.
    #[test]
    fn purge_deletes_the_chat_keys_on_a_lab_store_and_keeps_the_account() {
        use matrix_sdk::SqliteCryptoStore;
        use matrix_sdk_crypto::store::types::BackupDecryptionKey;
        use matrix_sdk_crypto::types::events::room_key_request::{MegolmV1AesSha2Content, SupportedKeyInfo};
        let rt = rt();
        let dir = tmpdir("purge");
        let room = room_id!("!chat:lab.test");
        rt.block_on(async {
            let s = SqliteCryptoStore::open(&dir, Some("lab-passphrase")).await.unwrap();
            let acct = Account::with_device_id(user_id!("@recorder:lab.test"), device_id!("RECORDERDEV"));
            s.save_pending_changes(PendingChanges { account: Some(acct) }).await.unwrap();
            let (out, inb) = pair(room).await;
            let mut c = Changes::default();
            c.inbound_group_sessions = vec![inb, inbound(room).await, inbound(room).await];
            c.outbound_group_sessions = vec![out];
            c.backup_decryption_key = Some(BackupDecryptionKey::from_bytes(&[7u8; 32]));
            c.key_requests = vec![GossipRequest {
                request_recipient: user_id!("@recorder:lab.test").to_owned(),
                request_id: TransactionId::new(),
                info: SecretInfo::KeyRequest(SupportedKeyInfo::MegolmV1AesSha2(MegolmV1AesSha2Content {
                    room_id: room.to_owned(),
                    sender_key: None,
                    session_id: "s".into(),
                })),
                sent_out: true,
            }];
            c.next_batch_token = Some("s9".into());
            s.save_changes(c).await.unwrap();
        });
        let db = dir.join(CRYPTO_DB);
        assert_eq!(chat_key_counts(&db).unwrap(), (3, 3), "the lab store: 3 sessions + outbound, request, backup key");
        let p = purge(&dir).unwrap();
        assert_eq!(p, Purged { before: (3, 3), after: (0, 0), cleanup: None });
        assert_eq!(chat_key_counts(&db).unwrap(), (0, 0), "purged");
        rt.block_on(async {
            let s = SqliteCryptoStore::open(&dir, Some("lab-passphrase")).await.unwrap();
            assert!(s.load_account().await.unwrap().is_some(), "the recorder's Olm account survives the purge");
            assert_eq!(s.next_batch_token().await.unwrap().as_deref(), Some("s9"));
            assert_eq!(s.inbound_group_session_counts(None).await.unwrap().total, 0);
            assert!(s.load_backup_keys().await.unwrap().decryption_key.is_none());
        });
        assert_eq!(purge(&tmpdir("purge-none")).unwrap(), Purged { before: (0, 0), after: (0, 0), cleanup: None }, "no store yet");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A store another process holds (writing, or only reading): the purge refuses before deleting anything.
    #[test]
    fn purge_refuses_a_store_in_use() {
        use matrix_sdk::SqliteCryptoStore;
        let rt = rt();
        let dir = tmpdir("purge-busy");
        let room = room_id!("!chat:lab.test");
        rt.block_on(async {
            let s = SqliteCryptoStore::open(&dir, None).await.unwrap();
            let mut c = Changes::default();
            c.inbound_group_sessions = vec![inbound(room).await];
            s.save_changes(c).await.unwrap();
        });
        for (what, hold) in [("a writer", "BEGIN EXCLUSIVE;"), ("a reader", "BEGIN; SELECT COUNT(*) FROM inbound_group_session;")] {
            let holder = rusqlite::Connection::open(dir.join(CRYPTO_DB)).unwrap();
            holder.execute_batch(hold).unwrap();
            let t0 = std::time::Instant::now();
            let r = purge(&dir);
            assert!(r.is_err(), "refused while {what} holds the store: {r:?}");
            assert!(t0.elapsed().as_secs() >= 4, "it waited for the busy timeout first ({what})");
            holder.execute_batch("ROLLBACK;").unwrap();
            drop(holder);
            assert_eq!(chat_key_counts(&dir.join(CRYPTO_DB)).unwrap().0, 1, "nothing was deleted ({what})");
        }
        assert_eq!(purge(&dir).unwrap().after, (0, 0), "once the store is free, the purge runs");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_purge_is_opt_in() {
        std::env::remove_var(PURGE_ENV);
        assert!(!purge_wanted(), "default OFF");
        for v in ["0", "", "yes", "true", "11"] {
            std::env::set_var(PURGE_ENV, v);
            assert!(!purge_wanted(), "{v:?} is not 1");
        }
        std::env::set_var(PURGE_ENV, "1");
        assert!(purge_wanted());
        std::env::remove_var(PURGE_ENV);
    }
}
