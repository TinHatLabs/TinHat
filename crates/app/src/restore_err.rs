//! restore_err — why a saved session didn't open, and what the sign-in card offers instead (desktop-gates-night,
//! 2026-10-28). Until 12.1.1 every restore error deleted session.json at once: a store written by a newer Tin Hat (a
//! downgrade), a keyring that didn't answer (M10) and a damaged file all ended in "sign in again" = a new device, and
//! "unlock your keyring and open Tin Hat again" could never work. Now nothing is deleted on a failed restore; the card
//! says which case it is and offers the way out, and only the person's own "Start fresh" forgets the session.
//!
//! - **newer**: the store's databases carry a schema version this build doesn't know (an older Tin Hat on a newer
//!   store; matrix-sdk-sqlite opens it anyway and fails later on a record it can't read). Offer Quit / Start fresh.
//! - **keyring**: the store is locked with a key in the system keyring and the keyring didn't answer. Offer Try again.
//! - **unreadable**: anything else (a damaged file, a lost key or marker). Offer Try again / Start fresh.
//!
//! Detecting "newer" needs no help from the newer build: after a failed restore, the store's own version records
//! (matrix-sdk-sqlite's kv key `version`, plain even in an encrypted store) are compared with what this build's
//! matrix-sdk writes into a throwaway store. The version named in the words comes from `<data>/store-writer.json`, the
//! newest Tin Hat that opened this store (12.1.1 and later write it).
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// QML `restoreProblem` values.
pub const KIND_NEWER: &str = "newer";
pub const KIND_KEYRING: &str = "keyring";
pub const KIND_UNREADABLE: &str = "unreadable";

/// storekey.rs's KEYRING_UNAVAILABLE says this (M10): the store is fine, the keyring didn't answer.
const KEYRING_MARK: &str = "the keyring didn't answer";
/// storekey.rs's words all start like this: they already say what to do, so the card shows them as they are.
const STOREKEY_PREFIX: &str = "Tin Hat's chat store on this computer is locked";

const WRITER_FILE: &str = "store-writer.json";
/// The databases matrix-sdk-sqlite keeps in the store directory (as storekey.rs).
const STORE_DBS: &[&str] = &["matrix-sdk-state.sqlite3", "matrix-sdk-crypto.sqlite3", "matrix-sdk-event-cache.sqlite3", "matrix-sdk-media.sqlite3"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// written by a newer Tin Hat; `version` = the newest one that opened it, when it said so
    Newer { version: Option<String> },
    /// the keyring didn't answer (`text` = storekey's words)
    Keyring { text: String },
    /// anything else (`text` = words of the error's own when it has them, else None)
    Unreadable { text: Option<String> },
}

impl Problem {
    pub fn kind(&self) -> &'static str {
        match self {
            Problem::Newer { .. } => KIND_NEWER,
            Problem::Keyring { .. } => KIND_KEYRING,
            Problem::Unreadable { .. } => KIND_UNREADABLE,
        }
    }

    /// The sign-in card's line.
    pub fn text(&self) -> String {
        match self {
            Problem::Newer { version: Some(v) } => format!("Your data was saved by a newer Tin Hat. Install Tin Hat {v} or newer to keep this sign-in."),
            Problem::Newer { version: None } => "Your data was saved by a newer Tin Hat. Install the newest Tin Hat to keep this sign-in.".to_string(),
            Problem::Keyring { text } => text.clone(),
            Problem::Unreadable { text: Some(t) } => t.clone(),
            Problem::Unreadable { text: None } => "Your saved session couldn't be opened. Nothing was deleted: try again, or start fresh on this computer (you sign in again; your encrypted messages come back with your Security Key or another signed-in device).".to_string(),
        }
    }
}

/// The decision (pure): a store newer than this build wins (whatever the error said, that is the cause); then the
/// keyring; everything else is unreadable.
pub fn classify(err: &str, store_newer: bool, writer: Option<&str>, me: &str) -> Problem {
    if store_newer {
        // name a version only when it really is newer than this build (an older or equal record says nothing)
        let version = writer.filter(|w| version_newer(w, me)).map(str::to_string);
        return Problem::Newer { version };
    }
    if err.contains(KEYRING_MARK) {
        return Problem::Keyring { text: err.to_string() };
    }
    if err.starts_with(STOREKEY_PREFIX) {
        return Problem::Unreadable { text: Some(err.to_string()) };
    }
    Problem::Unreadable { text: None }
}

/// "12.10.0" > "12.9.3" (numeric parts; a missing part = 0; anything after a '-' or '+' is ignored; unparsable = 0).
pub fn version_newer(a: &str, b: &str) -> bool {
    fn parts(v: &str) -> Vec<u64> {
        let core = v.trim().split(['-', '+']).next().unwrap_or("");
        core.split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect()
    }
    let (pa, pb) = (parts(a), parts(b));
    for i in 0..pa.len().max(pb.len()) {
        let (x, y) = (pa.get(i).copied().unwrap_or(0), pb.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

// ---- store-writer.json: the newest Tin Hat that opened this store ----------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize)]
struct Writer {
    v: u32,
    app: String,
}

fn writer_path(data: &Path) -> PathBuf {
    data.join(WRITER_FILE)
}

pub fn read_writer(data: &Path) -> Option<String> {
    let s = std::fs::read_to_string(writer_path(data)).ok()?;
    serde_json::from_str::<Writer>(&s).ok().map(|w| w.app).filter(|a| !a.is_empty())
}

/// After a restore or sign-in that opened the store: record `me`, unless a newer Tin Hat already did (an older build
/// that opens a newer store fine leaves the record alone).
pub fn note_writer(data: &Path, me: &str) {
    if read_writer(data).is_some_and(|w| !version_newer(me, &w)) {
        return;
    }
    let body = serde_json::to_string(&Writer { v: 1, app: me.to_string() }).unwrap_or_default();
    let tmp = data.join(format!("{WRITER_FILE}.tmp"));
    if std::fs::write(&tmp, body).is_ok() {
        let _ = std::fs::rename(&tmp, writer_path(data));
    }
}

/// Forget the record (Start fresh, sign-in wipes): the next store is a new one.
pub fn forget_writer(data: &Path) {
    let _ = std::fs::remove_file(writer_path(data));
}

// ---- schema versions ---------------------------------------------------------------------------------------------

/// matrix-sdk-sqlite's `version` record of each database in `store` (read-only). The SDK writes exactly ONE byte and
/// rejects anything else as InvalidVersion, so a database that is missing, has no record or a malformed one is left
/// out (it can only be "unreadable", never "newer" — Astra 2026-10-28).
pub fn store_versions(store: &Path) -> BTreeMap<String, u64> {
    use rusqlite::{Connection, OpenFlags};
    let mut out = BTreeMap::new();
    for name in STORE_DBS {
        let p = store.join(name);
        if !p.exists() {
            continue;
        }
        let Ok(c) = Connection::open_with_flags(&p, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX) else { continue };
        let v: Option<Vec<u8>> = c.query_row("SELECT value FROM kv WHERE key = 'version'", [], |r| r.get(0)).ok();
        if let Some([b]) = v.as_deref() {
            out.insert(name.to_string(), u64::from(*b));
        }
    }
    out
}

/// Is any database on disk newer than the same database this build writes? (pure)
pub fn newer_than(disk: &BTreeMap<String, u64>, mine: &BTreeMap<String, u64>) -> bool {
    disk.iter().any(|(k, v)| mine.get(k).is_some_and(|m| v > m))
}

/// What this build's matrix-sdk writes: a throwaway store in a private temp dir, read back, removed.
pub async fn own_versions() -> BTreeMap<String, u64> {
    use rand::Rng;
    let dir = std::env::temp_dir().join(format!("tinhat-store-probe-{}-{:08x}", std::process::id(), rand::thread_rng().gen::<u32>()));
    let mut out = BTreeMap::new();
    if std::fs::create_dir(&dir).is_ok() {
        let built = matrix_sdk::Client::builder().homeserver_url("http://127.0.0.1:9").sqlite_store(&dir, None).build().await;
        if let Ok(client) = built {
            drop(client);
            out = store_versions(&dir);
        }
        // the SDK closes its pooled SQLite handles on a background task: on Windows an open handle blocks the delete,
        // so retry briefly (an empty probe store; never the person's data)
        for i in 0..20 {
            match std::fs::remove_dir_all(&dir) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    if i == 19 {
                        eprintln!("[restore] couldn't remove the throwaway store {}: {e}", dir.display());
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                _ => break,
            }
        }
    }
    out
}

/// After a failed restore: which problem is it? (reads the store, makes a throwaway one; deletes nothing of the user's)
pub async fn diagnose(err: &str, data: &Path, store: &Path, me: &str) -> Problem {
    let disk = store_versions(store);
    let newer = !disk.is_empty() && newer_than(&disk, &own_versions().await);
    if newer {
        eprintln!("[restore] the store was written by a newer Tin Hat: {disk:?}");
    }
    classify(err, newer, read_writer(data).as_deref(), me)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = "12.1.1";
    const KEYRING: &str = "Tin Hat's chat store on this computer is locked with a key in your system keyring, and the keyring didn't answer. Unlock it (or start your desktop's keyring) and open Tin Hat again.";
    const KEY_GONE: &str = "Tin Hat's chat store on this computer is locked with a key that is no longer in your system keyring. Sign in again to set this computer up fresh.";

    #[test]
    fn the_decision_table() {
        // a newer store wins over the error's words
        assert_eq!(classify("invalid type: map, expected a sequence", true, Some("12.2.0"), ME), Problem::Newer { version: Some("12.2.0".into()) });
        assert_eq!(classify(KEYRING, true, None, ME), Problem::Newer { version: None });
        // a record that isn't newer than this build names no version
        assert_eq!(classify("x", true, Some("12.1.1"), ME), Problem::Newer { version: None });
        assert_eq!(classify("x", true, Some("12.0.5"), ME), Problem::Newer { version: None });
        // the keyring didn't answer: its own words, Try again
        assert_eq!(classify(KEYRING, false, None, ME), Problem::Keyring { text: KEYRING.into() });
        // a lost key / marker: storekey's words, unreadable (Start fresh is the way out)
        assert_eq!(classify(KEY_GONE, false, None, ME), Problem::Unreadable { text: Some(KEY_GONE.into()) });
        // anything else
        assert_eq!(classify("expected value at line 1 column 1", false, Some("12.2.0"), ME), Problem::Unreadable { text: None });
        assert_eq!(classify("", false, None, ME), Problem::Unreadable { text: None });
    }

    #[test]
    fn kinds_and_words() {
        assert_eq!(Problem::Newer { version: None }.kind(), "newer");
        assert_eq!(Problem::Keyring { text: KEYRING.into() }.kind(), "keyring");
        assert_eq!(Problem::Unreadable { text: None }.kind(), "unreadable");
        assert_eq!(
            Problem::Newer { version: Some("12.2.0".into()) }.text(),
            "Your data was saved by a newer Tin Hat. Install Tin Hat 12.2.0 or newer to keep this sign-in."
        );
        assert!(Problem::Newer { version: None }.text().contains("newest Tin Hat"));
        assert_eq!(Problem::Keyring { text: KEYRING.into() }.text(), KEYRING);
        assert!(Problem::Unreadable { text: None }.text().contains("Nothing was deleted"));
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(version_newer("12.10.0", "12.9.3"));
        assert!(version_newer("12.1.1", "12.1.0"));
        assert!(version_newer("13", "12.99.99"));
        assert!(!version_newer("12.1.0", "12.1.0"));
        assert!(!version_newer("12.1", "12.1.0"));
        assert!(!version_newer("12.1.0-rc1", "12.1.0"));
        assert!(!version_newer("garbage", "0.0.1"));
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("restore-err-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_writer_record_keeps_the_newest() {
        let d = tmp("writer");
        assert_eq!(read_writer(&d), None);
        note_writer(&d, "12.1.1");
        assert_eq!(read_writer(&d).as_deref(), Some("12.1.1"));
        note_writer(&d, "12.1.0"); // an older build that opened it fine leaves the record alone
        assert_eq!(read_writer(&d).as_deref(), Some("12.1.1"));
        note_writer(&d, "12.2.0");
        assert_eq!(read_writer(&d).as_deref(), Some("12.2.0"));
        std::fs::write(d.join(WRITER_FILE), "not json").unwrap();
        assert_eq!(read_writer(&d), None);
        note_writer(&d, "12.1.1"); // a damaged record is replaced
        assert_eq!(read_writer(&d).as_deref(), Some("12.1.1"));
        forget_writer(&d);
        assert_eq!(read_writer(&d), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    fn fake_db(dir: &Path, name: &str, version: &[u8]) {
        let c = rusqlite::Connection::open(dir.join(name)).unwrap();
        c.execute_batch("CREATE TABLE kv (key TEXT PRIMARY KEY NOT NULL, value BLOB NOT NULL);").unwrap();
        c.execute("INSERT INTO kv VALUES ('version', ?1)", [version]).unwrap();
    }

    #[test]
    fn store_versions_are_read_and_compared() {
        let d = tmp("versions");
        fake_db(&d, "matrix-sdk-state.sqlite3", &[0x10]);
        fake_db(&d, "matrix-sdk-crypto.sqlite3", &[0x13]);
        let c = rusqlite::Connection::open(d.join("matrix-sdk-media.sqlite3")).unwrap(); // no kv table: left out
        c.execute_batch("CREATE TABLE other (x);").unwrap();
        fake_db(&d, "matrix-sdk-event-cache.sqlite3", &[0x31, 0x39]); // malformed (the SDK writes one byte): left out
        let disk = store_versions(&d);
        assert_eq!(disk.get("matrix-sdk-state.sqlite3"), Some(&16));
        assert_eq!(disk.get("matrix-sdk-crypto.sqlite3"), Some(&19));
        assert_eq!(disk.len(), 2);
        let mut mine = disk.clone();
        assert!(!newer_than(&disk, &mine));
        mine.insert("matrix-sdk-state.sqlite3".into(), 15); // 12.1.0's state store on 12.1.1's
        assert!(newer_than(&disk, &mine));
        mine.insert("matrix-sdk-state.sqlite3".into(), 17);
        assert!(!newer_than(&disk, &mine));
        assert!(!newer_than(&disk, &BTreeMap::new())); // nothing to compare with = not newer
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn this_builds_own_store_is_never_newer_than_itself() {
        // the app installs its TLS provider at start (main.rs); a test run alone must do it too (as storekey/update tests)
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let mine = rt.block_on(own_versions());
        assert!(mine.contains_key("matrix-sdk-state.sqlite3") && mine.contains_key("matrix-sdk-crypto.sqlite3"), "{mine:?}");
        assert!(!newer_than(&mine, &mine));
        let probe_left = std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with(&format!("tinhat-store-probe-{}-", std::process::id())));
        assert!(!probe_left, "the throwaway store must be removed");
    }
}
