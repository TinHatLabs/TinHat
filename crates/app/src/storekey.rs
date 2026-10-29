//! storekey — M10 (security review 2026-10-26; sec-night 2026-10-27): the E2EE / state store's passphrase in the OS
//! keyring, so a copy of the matrix-sdk store (a backup, a disk image, another admin) doesn't give away the account's
//! encryption keys and stored state without this user's keyring. NOT covered: session.json (the access token, still a
//! 0600 file), the app's own caches outside the store (timeline / members / media / link-preview caches under the
//! cache dir), logs, older plain copies, anything a program running as this user can ask the keyring for. matrix-sdk
//! encrypts record values, not whole files (table layout and some keys stay visible; custom values stay plain by its
//! design).
//! Plan + reasons: share tinhat-security-review/fixes/desktop-trust.md "M10 implementation plan".
//!
//! OFF by default: `TINHAT_STORE_KEYRING=1` turns it on, and only for a FRESH store (every sign-in wipes the store
//! first). matrix-sdk can't encrypt an existing plain store in place, so an existing store is never touched.
//!   - `<data>/store-key.json` (the marker, 0600) says "this store is encrypted, its key is keyring item <id>"; it is
//!     written only after the keyring took the key, before the store is created. No marker = a plain store (today's
//!     behaviour), flag or not — unless the store holds a cipher (marker lost): then it is refused, never opened plain.
//!   - the key: 32 random bytes, base64 = the `sqlite_store(dir, Some(pass))` passphrase (matrix-sdk derives the store
//!     cipher of the state, crypto and event-cache stores from it).
//!   - the keyring (Linux): oo7 — the Secret Service on the host, the secret portal inside Flatpak. Windows / macOS: not
//!     yet ([`NoKeys`]: the flag is ignored there, logged; an encrypted store is refused).
//!   - making a key fails → a plain store (sign-in never blocks); a marked store whose key can't be read → the restore
//!     fails with words that say what to do, nothing is deleted.
use base64::Engine as _;
use std::path::{Path, PathBuf};

/// The flag (until the feature has had its lab run): `TINHAT_STORE_KEYRING=1`.
pub const FLAG_ENV: &str = "TINHAT_STORE_KEYRING";
const MARKER_FILE: &str = "store-key.json";
const LABEL: &str = "Tin Hat chat store key";
const APP_ID: &str = "dev.tinhat.TinHat";

/// Restore: the store is encrypted but the keyring didn't answer (locked, not running, the unlock was cancelled).
pub const KEYRING_UNAVAILABLE: &str = "Tin Hat's chat store on this computer is locked with a key in your system keyring, and the keyring didn't answer. Unlock it (or start your desktop's keyring) and open Tin Hat again.";
/// Restore: the store is encrypted and its key is no longer in the keyring.
pub const KEY_GONE: &str = "Tin Hat's chat store on this computer is locked with a key that is no longer in your system keyring. Sign in again to set this computer up fresh.";
/// Restore: the marker can't be read, or it is gone while the store is still encrypted.
pub const MARKER_DAMAGED: &str = "Tin Hat's chat store on this computer is locked, and the note saying which key opens it is missing or damaged. Sign in again to set this computer up fresh.";

/// `<data>/store-key.json`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Marker {
    pub v: u32,
    /// the keyring item's `id` attribute (32 lower-case hex)
    pub id: String,
    pub backend: String,
}

/// What the store builder does (pure; [`plan`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// no passphrase (today's store)
    Plain,
    /// a fresh store and the flag on: make a key, keep it in the keyring, then mark the store
    MakeKey,
    /// a marked store: its key comes from the keyring item with this id
    NeedKey(String),
    /// an encrypted store without its marker: never open it as a plain one (matrix-sdk would, silently)
    Refuse,
}

/// The decision table: a marked store always needs its key (the flag no longer matters once a store is encrypted); an
/// existing unmarked store stays plain unless it is encrypted (its marker lost: refused); a fresh store gets a key only
/// with the flag on, on a platform with a keyring.
pub fn plan(marker: Option<&Marker>, store_exists: bool, store_encrypted: bool, flag_on: bool, supported: bool) -> Plan {
    match marker {
        Some(m) => Plan::NeedKey(m.id.clone()),
        None if store_exists && store_encrypted => Plan::Refuse,
        None if !store_exists && flag_on && supported => Plan::MakeKey,
        None => Plan::Plain,
    }
}

/// The databases matrix-sdk-sqlite keeps in the store directory.
const STORE_DBS: &[&str] = &["matrix-sdk-state.sqlite3", "matrix-sdk-crypto.sqlite3", "matrix-sdk-event-cache.sqlite3", "matrix-sdk-media.sqlite3"];

/// Does any database in `store` hold a store cipher (matrix-sdk-sqlite's kv key `cipher`, written only when the store
/// was opened with a passphrase)? Read-only; a database without a kv table counts as plain. Err = one exists but
/// can't be read (then nothing is opened).
pub fn store_encrypted(store: &Path) -> Result<bool, String> {
    use rusqlite::{Connection, OpenFlags};
    for name in STORE_DBS {
        let p = store.join(name);
        if !p.exists() {
            continue;
        }
        let c = Connection::open_with_flags(&p, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|e| format!("{name}: {e}"))?;
        let has_kv: bool = c
            .query_row("SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'kv'", [], |r| r.get::<_, i64>(0))
            .map_err(|e| format!("{name}: {e}"))?
            > 0;
        if has_kv {
            let n: i64 = c.query_row("SELECT count(*) FROM kv WHERE key = 'cipher'", [], |r| r.get(0)).map_err(|e| format!("{name}: {e}"))?;
            if n > 0 {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Is the flag on in this process?
pub fn flag_on() -> bool {
    matches!(std::env::var(FLAG_ENV).as_deref().map(str::trim), Ok("1" | "true" | "on" | "yes"))
}

/// Is there a keyring backend on this platform?
pub fn supported() -> bool {
    cfg!(target_os = "linux")
}

fn marker_path(data: &Path) -> PathBuf {
    data.join(MARKER_FILE)
}

/// The marker: Ok(None) when there is none, Err when it exists but can't be read as one.
pub fn read_marker(data: &Path) -> Result<Option<Marker>, String> {
    match std::fs::read_to_string(marker_path(data)) {
        Ok(s) => {
            let m: Marker = serde_json::from_str(&s).map_err(|_| MARKER_DAMAGED.to_string())?;
            let ok = m.v == 1 && m.id.len() == 32 && m.id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase());
            if ok {
                Ok(Some(m))
            } else {
                Err(MARKER_DAMAGED.to_string())
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(MARKER_DAMAGED.to_string()),
    }
}

fn write_marker(data: &Path, m: &Marker) -> std::io::Result<()> {
    std::fs::create_dir_all(data)?;
    let json = serde_json::to_vec(m).map_err(std::io::Error::other)?;
    crate::platform::write_private_file(&marker_path(data), &json)
}

/// Wiping the store: the marker goes with it; returns the keyring item id to delete (the caller does it in the
/// background — a stale random key unlocks nothing once its store is gone).
pub fn forget_marker(data: &Path) -> Option<String> {
    let id = read_marker(data).ok().flatten().map(|m| m.id);
    let _ = std::fs::remove_file(marker_path(data));
    id
}

/// Where keys are kept (the OS keyring; tests use an in-memory one).
#[allow(async_fn_in_trait)]
pub trait KeyStore {
    fn backend(&self) -> &'static str;
    async fn put(&self, id: &str, secret: &str) -> Result<(), String>;
    /// Ok(None) = the keyring answered and has no such item
    async fn get(&self, id: &str) -> Result<Option<String>, String>;
    async fn delete(&self, id: &str) -> Result<(), String>;
}

fn random_hex(n: usize) -> String {
    use rand::RngCore;
    let mut b = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn random_key() -> String {
    use rand::RngCore;
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    base64::engine::general_purpose::STANDARD_NO_PAD.encode(b)
}

/// The passphrase for the store the caller is about to open (None = a plain store), following [`plan`]. `store` = the
/// store directory (it exists = it has anything in it).
pub async fn passphrase<K: KeyStore>(data: &Path, store: &Path, flag_on: bool, supported: bool, keys: &K) -> Result<Option<String>, String> {
    let marker = read_marker(data)?;
    let store_exists = std::fs::read_dir(store).map(|mut d| d.next().is_some()).unwrap_or(false);
    let encrypted = if marker.is_none() && store_exists {
        store_encrypted(store).map_err(|e| {
            eprintln!("[storekey] can't tell whether the store is encrypted ({e}): not opening it");
            MARKER_DAMAGED.to_string()
        })?
    } else {
        false
    };
    match plan(marker.as_ref(), store_exists, encrypted, flag_on, supported) {
        Plan::Refuse => {
            eprintln!("[storekey] the store is encrypted but its marker is gone: not opening it as a plain store");
            Err(MARKER_DAMAGED.to_string())
        }
        Plan::Plain => {
            if flag_on && !supported {
                eprintln!("[storekey] {FLAG_ENV} is set, but this platform has no keyring support yet: plain store");
            }
            Ok(None)
        }
        Plan::NeedKey(id) => match keys.get(&id).await {
            Ok(Some(pass)) => Ok(Some(pass)),
            Ok(None) => {
                eprintln!("[storekey] the store's key {id} is not in the keyring");
                Err(KEY_GONE.to_string())
            }
            Err(e) => {
                eprintln!("[storekey] reading the store's key failed: {e}");
                Err(KEYRING_UNAVAILABLE.to_string())
            }
        },
        Plan::MakeKey => {
            let id = random_hex(16);
            let pass = random_key();
            if let Err(e) = keys.put(&id, &pass).await {
                eprintln!("[storekey] the keyring didn't take a key ({e}): this store stays unencrypted");
                return Ok(None);
            }
            let m = Marker { v: 1, id: id.clone(), backend: keys.backend().to_string() };
            if let Err(e) = write_marker(data, &m) {
                eprintln!("[storekey] writing the store marker failed ({e}): this store stays unencrypted");
                let _ = keys.delete(&id).await;
                return Ok(None);
            }
            eprintln!("[storekey] new store encrypted with keyring item {id} ({})", keys.backend());
            Ok(Some(pass))
        }
    }
}

/// The OS keyring (Linux): oo7 — the Secret Service, or the secret portal inside Flatpak.
#[cfg(target_os = "linux")]
pub struct Oo7;

#[cfg(target_os = "linux")]
impl Oo7 {
    async fn keyring() -> Result<oo7::Keyring, String> {
        let k = oo7::Keyring::new().await.map_err(|e| e.to_string())?;
        k.unlock().await.map_err(|e| e.to_string())?;
        Ok(k)
    }
}

#[cfg(target_os = "linux")]
fn attrs(id: &str) -> [(&'static str, &str); 3] {
    [("application", APP_ID), ("purpose", "store-key"), ("id", id)]
}

#[cfg(target_os = "linux")]
impl KeyStore for Oo7 {
    fn backend(&self) -> &'static str {
        "secret-service"
    }
    async fn put(&self, id: &str, secret: &str) -> Result<(), String> {
        let k = Self::keyring().await?;
        k.create_item(LABEL, &attrs(id), secret, true).await.map_err(|e| e.to_string())
    }
    async fn get(&self, id: &str) -> Result<Option<String>, String> {
        let k = Self::keyring().await?;
        let items = k.search_items(&attrs(id)).await.map_err(|e| e.to_string())?;
        let Some(item) = items.first() else { return Ok(None) };
        if item.is_locked().await.map_err(|e| e.to_string())? {
            item.unlock().await.map_err(|e| e.to_string())?;
        }
        let s = item.secret().await.map_err(|e| e.to_string())?;
        String::from_utf8(s.as_bytes().to_vec()).map(Some).map_err(|_| "the stored key isn't text".to_string())
    }
    async fn delete(&self, id: &str) -> Result<(), String> {
        let k = Self::keyring().await?;
        k.delete(&attrs(id)).await.map_err(|e| e.to_string())
    }
}

/// The keyring this build uses: oo7 on Linux.
#[cfg(target_os = "linux")]
pub fn os_keys() -> Oo7 {
    Oo7
}

/// No keyring backend on this platform yet (Windows / macOS): no key is ever made ([`supported`] is false); a marked
/// store can't be opened ([`KEYRING_UNAVAILABLE`]) and an encrypted unmarked one is refused — never opened as plain.
#[cfg(not(target_os = "linux"))]
pub struct NoKeys;

#[cfg(not(target_os = "linux"))]
impl KeyStore for NoKeys {
    fn backend(&self) -> &'static str {
        "none"
    }
    async fn put(&self, _id: &str, _secret: &str) -> Result<(), String> {
        Err("no keyring support on this platform yet".into())
    }
    async fn get(&self, _id: &str) -> Result<Option<String>, String> {
        Err("no keyring support on this platform yet".into())
    }
    async fn delete(&self, _id: &str) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
pub fn os_keys() -> NoKeys {
    NoKeys
}

/// Delete a wiped store's key in the background (best effort).
pub fn delete_in_background(id: String) {
    #[cfg(target_os = "linux")]
    crate::bridge::runtime().spawn(async move {
        if let Err(e) = Oo7.delete(&id).await {
            eprintln!("[storekey] removing the old store key {id} from the keyring failed: {e}");
        }
    });
    #[cfg(not(target_os = "linux"))]
    let _ = id;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// An in-memory keyring: `fail_put` / `fail_get` = the keyring doesn't answer.
    #[derive(Default)]
    struct MemKeys {
        items: RefCell<HashMap<String, String>>,
        fail_put: bool,
        fail_get: bool,
    }
    impl KeyStore for MemKeys {
        fn backend(&self) -> &'static str {
            "memory"
        }
        async fn put(&self, id: &str, secret: &str) -> Result<(), String> {
            if self.fail_put {
                return Err("no keyring".into());
            }
            self.items.borrow_mut().insert(id.into(), secret.into());
            Ok(())
        }
        async fn get(&self, id: &str) -> Result<Option<String>, String> {
            if self.fail_get {
                return Err("locked".into());
            }
            Ok(self.items.borrow().get(id).cloned())
        }
        async fn delete(&self, id: &str) -> Result<(), String> {
            self.items.borrow_mut().remove(id);
            Ok(())
        }
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
    }

    /// A store directory with something in it (a restore).
    fn existing(d: &Path) -> PathBuf {
        let s = d.join("store");
        std::fs::create_dir_all(&s).unwrap();
        std::fs::write(s.join("x"), b"x").unwrap();
        s
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tinhat-storekey-{tag}-{}", random_hex(6)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_decision_table() {
        let m = Marker { v: 1, id: "0".repeat(32), backend: "x".into() };
        // a marked store always needs its key, whatever the flag / platform says
        for (exists, flag, sup) in [(true, true, true), (true, false, false), (false, false, true), (false, true, false)] {
            assert_eq!(plan(Some(&m), exists, false, flag, sup), Plan::NeedKey(m.id.clone()));
        }
        assert_eq!(plan(None, false, false, true, true), Plan::MakeKey, "fresh + flag + keyring");
        assert_eq!(plan(None, true, false, true, true), Plan::Plain, "an existing plain store is never encrypted in place");
        assert_eq!(plan(None, false, false, false, true), Plan::Plain, "flag off");
        assert_eq!(plan(None, false, false, true, false), Plan::Plain, "no keyring on this platform");
        for (flag, sup) in [(true, true), (false, false)] {
            assert_eq!(plan(None, true, true, flag, sup), Plan::Refuse, "an encrypted store whose marker is gone");
        }
    }

    #[test]
    fn a_fresh_store_gets_a_key_and_the_next_start_reads_it_back() {
        let d = tmp("make");
        let keys = MemKeys::default();
        let pass = rt().block_on(passphrase(&d, &d.join("store"), true, true, &keys)).unwrap().expect("a passphrase");
        assert!(pass.len() >= 43, "32 random bytes, base64");
        let m = read_marker(&d).unwrap().expect("marker written");
        assert_eq!(m.backend, "memory");
        assert_eq!(keys.items.borrow().get(&m.id), Some(&pass));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(d.join(MARKER_FILE)).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // the next start: the store exists now; the flag turned off doesn't matter any more
        assert_eq!(rt().block_on(passphrase(&d, &existing(&d), false, true, &keys)).unwrap(), Some(pass));
        // two fresh stores never share a key
        let d2 = tmp("make2");
        let p2 = rt().block_on(passphrase(&d2, &d2.join("store"), true, true, &keys)).unwrap().unwrap();
        assert_ne!(p2, keys.items.borrow().get(&m.id).cloned().unwrap());
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&d2);
    }

    #[test]
    fn no_keyring_means_a_plain_store_and_no_marker() {
        let d = tmp("nokeyring");
        let keys = MemKeys { fail_put: true, ..Default::default() };
        assert_eq!(rt().block_on(passphrase(&d, &d.join("store"), true, true, &keys)).unwrap(), None);
        assert_eq!(read_marker(&d).unwrap(), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_marked_store_never_opens_without_its_key() {
        let d = tmp("restore");
        let keys = MemKeys::default();
        rt().block_on(passphrase(&d, &d.join("store"), true, true, &keys)).unwrap().unwrap();
        let locked = MemKeys { fail_get: true, items: RefCell::new(keys.items.borrow().clone()), ..Default::default() };
        assert_eq!(rt().block_on(passphrase(&d, &existing(&d), true, true, &locked)), Err(KEYRING_UNAVAILABLE.to_string()));
        let gone = MemKeys::default();
        assert_eq!(rt().block_on(passphrase(&d, &existing(&d), true, true, &gone)), Err(KEY_GONE.to_string()));
        assert!(read_marker(&d).unwrap().is_some(), "nothing is deleted on a failed restore");
        std::fs::write(d.join(MARKER_FILE), b"{not json").unwrap();
        assert_eq!(rt().block_on(passphrase(&d, &existing(&d), true, true, &keys)), Err(MARKER_DAMAGED.to_string()));
        std::fs::write(d.join(MARKER_FILE), br#"{"v":1,"id":"../../etc","backend":"x"}"#).unwrap();
        assert_eq!(read_marker(&d), Err(MARKER_DAMAGED.to_string()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn wiping_forgets_the_marker_and_names_the_key_to_delete() {
        let d = tmp("wipe");
        let keys = MemKeys::default();
        rt().block_on(passphrase(&d, &d.join("store"), true, true, &keys)).unwrap().unwrap();
        let id = read_marker(&d).unwrap().unwrap().id;
        assert_eq!(forget_marker(&d), Some(id));
        assert_eq!(read_marker(&d).unwrap(), None);
        assert_eq!(forget_marker(&d), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The real matrix-sdk sqlite store with the passphrase: nothing written through the client is readable in the
    /// files, the right key opens it again, a wrong key doesn't (and the plain store, the control, IS readable).
    #[test]
    fn the_sqlite_store_is_unreadable_without_its_key() {
        // (main() installs this before any client; with matrix-sdk 0.19 a client can't be built without it)
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        const PROBE: &[u8] = b"tinhat-storekey-plaintext-probe-7f3a";
        let grep = |dir: &Path| -> bool {
            std::fs::read_dir(dir).unwrap().flatten().any(|e| std::fs::read(e.path()).map(|b| b.windows(PROBE.len()).any(|w| w == PROBE)).unwrap_or(false))
        };
        let open = |dir: &Path, pass: Option<&str>| {
            let r = rt();
            r.block_on(async {
                let c = matrix_sdk::Client::builder().homeserver_url("http://127.0.0.1:1").sqlite_store(dir, pass).build().await;
                match c {
                    Ok(c) => {
                        // a kv entry the store encrypts (custom values are stored raw by matrix-sdk's design; Tin Hat
                        // keeps none there)
                        use matrix_sdk_base::store::{StateStoreDataKey, StateStoreDataValue};
                        let key = StateStoreDataKey::Filter("probe");
                        let got = c.state_store().get_kv_data(key).await.ok().flatten().and_then(|v| v.into_filter()).map(String::into_bytes);
                        if got.is_none() {
                            let v = StateStoreDataValue::Filter(String::from_utf8(PROBE.to_vec()).unwrap());
                            c.state_store().set_kv_data(key, v).await.unwrap();
                        }
                        drop(c);
                        Ok(got)
                    }
                    Err(e) => Err(e.to_string()),
                }
            })
        };
        let enc = tmp("sqlite-enc");
        let pass = random_key();
        assert_eq!(open(&enc, Some(&pass)).unwrap(), None);
        assert!(!grep(&enc), "the encrypted store's files hold the value in plain text");
        assert_eq!(open(&enc, Some(&pass)).unwrap().as_deref(), Some(PROBE), "the right key opens it");
        assert!(open(&enc, Some(&random_key())).is_err(), "a wrong key must not open it");
        // matrix-sdk itself opens an encrypted store with NO passphrase (it did in this test) — so storekey checks for
        // the stored cipher before opening an unmarked store as a plain one
        assert!(store_encrypted(&enc).unwrap(), "the encrypted store is recognised");
        let keys = MemKeys::default();
        let data = enc.parent().unwrap().join(format!("data-{}", random_hex(4)));
        std::fs::create_dir_all(&data).unwrap();
        assert_eq!(rt().block_on(passphrase(&data, &enc, true, true, &keys)), Err(MARKER_DAMAGED.to_string()), "marker lost: refused");
        let _ = std::fs::remove_dir_all(&data);
        let plain = tmp("sqlite-plain");
        assert_eq!(open(&plain, None).unwrap(), None);
        assert!(grep(&plain), "control: the plain store's files DO hold the value");
        assert!(!store_encrypted(&plain).unwrap(), "a plain store isn't mistaken for an encrypted one");
        let _ = std::fs::remove_dir_all(&enc);
        let _ = std::fs::remove_dir_all(&plain);
    }

    /// restore_err.rs (12.1.1) reads these words: the keyring that didn't answer is "Try again" (nothing deleted), a lost
    /// key or marker shows its own words with Try again / Start fresh. Change a message here → check the classifier.
    #[test]
    fn the_restore_card_knows_these_words() {
        use crate::restore_err::{classify, Problem};
        assert_eq!(classify(KEYRING_UNAVAILABLE, false, None, "12.1.1"), Problem::Keyring { text: KEYRING_UNAVAILABLE.into() });
        assert_eq!(classify(KEY_GONE, false, None, "12.1.1"), Problem::Unreadable { text: Some(KEY_GONE.into()) });
        assert_eq!(classify(MARKER_DAMAGED, false, None, "12.1.1"), Problem::Unreadable { text: Some(MARKER_DAMAGED.into()) });
    }
}
