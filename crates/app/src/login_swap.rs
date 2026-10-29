//! login_swap — a sign-in never destroys the saved session before the server said yes (login-order-night, 2026-10-28).
//! A password sign-in (and a new account, which signs in the same way) needs a clean store, and do_login used to wipe it
//! FIRST: a wrong password, a network failure or a cancel lost the saved session's store for nothing.
//!
//! The swap is one journaled transaction over the store dir and the files that belong to it (SIDE_FILES: session.json,
//! M10's store-key.json, restore_err's store-writer.json):
//!   prepare  `<store>.replaced-<ts>/` is made; every side file that exists is COPIED into its `_side/` (synced); the
//!            journal `<data>/login-swap.json` {state "pending", aside, side: [names that existed]} is written (synced).
//!   move     the store dir is RENAMED into the aside dir (`<aside>/store`): where it is says whether the move happened.
//!            The side files' originals stay where they are (the sign-in overwrites or removes them).
//!   sign in  into a fresh store; session.json is written by the sign-in.
//!   commit   the journal says "done" (synced) — only after the new session was saved; then the aside dir goes, then
//!            the journal. A failed save or a failed commit write = rollback.
//!   rollback put_back(): a fresh store is removed and `<aside>/store` renamed back (if it was moved); every side file is
//!            restored from its copy, every side file that didn't exist before is removed; then the aside dir, then the
//!            journal. The copies stay until the very end, so a rollback that stops half-way can simply run again.
//! At start ([`recover_on_start`], before restore opens anything): "pending" → rollback; "done" → finish the clean-up.
//! A pending journal blocks a new sign-in until it is resolved; a recovery that fails stops the restore with words.
//! An in-process flag keeps the start-up recovery away from a sign-in in flight.
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const JOURNAL: &str = "login-swap.json";
const SIDE_DIR: &str = "_side";
const MOVED_STORE: &str = "store";
/// Files in the data dir that belong to the saved session and travel with the transaction.
pub const SIDE_FILES: &[&str] = &["session.json", "store-key.json", "store-writer.json"];
const KEY_MARKER: &str = "store-key.json";

/// Data dirs with a sign-in swap running in this process (the start-up recovery must not touch those).
fn active() -> &'static Mutex<Vec<PathBuf>> {
    static A: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
    A.get_or_init(|| Mutex::new(Vec::new()))
}
fn set_active(data: &Path, on: bool) {
    let mut v = active().lock().unwrap_or_else(|e| e.into_inner());
    v.retain(|p| p != data);
    if on {
        v.push(data.to_path_buf());
    }
}
/// Check-and-set in one step: false = a sign-in already runs for this data dir.
fn try_set_active(data: &Path) -> bool {
    let mut v = active().lock().unwrap_or_else(|e| e.into_inner());
    if v.iter().any(|p| p == data) {
        return false;
    }
    v.push(data.to_path_buf());
    true
}
fn is_active(data: &Path) -> bool {
    active().lock().unwrap_or_else(|e| e.into_inner()).iter().any(|p| p == data)
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Journal {
    v: u32,
    state: String,
    aside: String,
    side: Vec<String>,
    /// keyring ids (M10) of fresh markers a rollback removed: kept here until the rollback is done, so a crash can't
    /// lose them
    #[serde(default)]
    forget: Vec<String>,
}

/// A store that was set aside for a sign-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aside {
    pub dir: PathBuf,
    data: PathBuf,
    store: PathBuf,
    side: Vec<String>,
}

fn journal_path(data: &Path) -> PathBuf {
    data.join(JOURNAL)
}

/// Make a directory's entries durable (Unix: fsync the directory; elsewhere a no-op). Errors count.
fn sync_dir(p: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(p)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = p;
    Ok(())
}

/// Write `bytes` to `to` durably: temp file, fsync, rename, fsync the directory.
fn write_durable(to: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = to.with_extension("tmp-swap");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, to)?;
    if let Some(d) = to.parent() {
        sync_dir(d)?;
    }
    Ok(())
}

fn write_journal(a: &Aside, state: &str, forget: &[String]) -> std::io::Result<()> {
    let j = Journal {
        v: 1,
        state: state.to_string(),
        aside: a.dir.file_name().unwrap_or_default().to_string_lossy().to_string(),
        side: a.side.clone(),
        forget: forget.to_vec(),
    };
    write_durable(&journal_path(&a.data), &serde_json::to_vec(&j).unwrap_or_default())
}

/// The journal: Ok(None) = none; Err = one exists but can't be read (then nothing may proceed).
fn journal(data: &Path) -> Result<Option<Journal>, String> {
    match std::fs::read(journal_path(data)) {
        Ok(b) => serde_json::from_slice(&b).map(Some).map_err(|_| "the sign-in journal is damaged".to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("the sign-in journal can't be read ({e})")),
    }
}

fn read_journal(data: &Path) -> Option<Journal> {
    journal(data).ok().flatten()
}

/// The keyring item id a marker names (M10's `{"v":1,"id":…}`), read as a plain file.
pub fn marker_id(path: &Path) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    v.get("id").and_then(|x| x.as_str()).map(str::to_string)
}

/// Remove a directory that a just-dropped client may still hold open (Windows): a few short retries.
fn remove_dir_retry(p: &Path) -> std::io::Result<()> {
    let mut last = Ok(());
    for _ in 0..20 {
        match std::fs::remove_dir_all(p) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                last = Err(e);
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
    last
}

/// What a sign-in starts with.
#[derive(Debug)]
pub enum Begin {
    /// a saved store was set aside (journaled); `forget` = keyring ids an earlier, resolved swap left to forget
    Aside(Aside, Vec<String>),
    /// there was no store: nothing to protect. `stray_keys` = keyring ids of markers left without a store and of an
    /// earlier, resolved swap (to forget)
    Fresh { stray_keys: Vec<String> },
}

/// prepare + move. A pending journal from an earlier sign-in is resolved first; if it can't be, the sign-in is refused.
pub fn begin(data: &Path, store: &Path) -> Result<Begin, String> {
    if !try_set_active(data) {
        return Err("Another sign-in is still running on this computer — wait for it and try again.".to_string());
    }
    let r = begin_inner(data, store);
    if !matches!(r, Ok(Begin::Aside(..))) {
        set_active(data, false); // only a set-aside store keeps the sign-in marked until commit / put_back
    }
    r
}

fn begin_inner(data: &Path, store: &Path) -> Result<Begin, String> {
    let mut forget = Vec::new();
    if !matches!(journal(data), Ok(None)) {
        match recover(data, store) {
            Recovered::Failed(e) => return Err(format!("Tin Hat couldn't finish an earlier sign-in on this computer ({e}). Nothing was deleted.")),
            Recovered::PutBack(k) => forget.extend(k),
            Recovered::Removed(Some(k)) => forget.push(k),
            _ => {}
        }
    }
    if !store.exists() {
        // no store to protect; a marker of a store that is gone would only confuse the fresh one
        let mut stray_keys: Vec<String> = marker_id(&data.join(KEY_MARKER)).into_iter().collect();
        stray_keys.extend(forget);
        for f in SIDE_FILES.iter().filter(|f| **f != "session.json") {
            let _ = std::fs::remove_file(data.join(f));
        }
        return Ok(Begin::Fresh { stray_keys });
    }
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let dir = store.with_file_name(format!("{}.replaced-{ts}", store.file_name().unwrap_or_default().to_string_lossy()));
    let fail = |e: std::io::Error, dir: &Path| {
        let _ = std::fs::remove_dir_all(dir);
        format!("couldn't set the saved session aside ({e})")
    };
    std::fs::create_dir_all(dir.join(SIDE_DIR)).map_err(|e| fail(e, &dir))?;
    let mut side = Vec::new();
    for f in SIDE_FILES {
        let from = data.join(f);
        if from.exists() {
            let bytes = std::fs::read(&from).map_err(|e| fail(e, &dir))?;
            write_durable(&dir.join(SIDE_DIR).join(f), &bytes).map_err(|e| fail(e, &dir))?;
            side.push(f.to_string());
        }
    }
    sync_dir(&dir).map_err(|e| fail(e, &dir))?;
    if let Some(p) = dir.parent() {
        sync_dir(p).map_err(|e| fail(e, &dir))?;
    }
    let a = Aside { dir: dir.clone(), data: data.to_path_buf(), store: store.to_path_buf(), side };
    write_journal(&a, "pending", &[]).map_err(|e| fail(e, &dir))?;
    if let Err(e) = std::fs::rename(store, dir.join(MOVED_STORE)) {
        // nothing moved: undo the preparation
        let _ = std::fs::remove_file(journal_path(data));
        let _ = std::fs::remove_dir_all(&dir);
        return Err(format!("couldn't set the saved session aside ({e})"));
    }
    let synced = sync_dir(&dir).and_then(|_| dir.parent().map_or(Ok(()), sync_dir));
    if let Err(e) = synced {
        let _ = put_back(&a);
        return Err(format!("couldn't set the saved session aside ({e})"));
    }
    // the side files of the old store would only confuse the fresh one (session.json stays until the sign-in writes it)
    for f in SIDE_FILES.iter().filter(|f| **f != "session.json") {
        let _ = std::fs::remove_file(data.join(f));
    }
    eprintln!("[login] the saved store is set aside ({}) until the server accepts the sign-in", dir.display());
    Ok(Begin::Aside(a, forget))
}

/// rollback (idempotent). Returns the keyring ids of FRESH markers it removed (M10), for the caller to forget.
pub fn put_back(a: &Aside) -> Result<Vec<String>, String> {
    let r = put_back_inner(a);
    set_active(&a.data, false);
    r
}

fn put_back_inner(a: &Aside) -> Result<Vec<String>, String> {
    let mut forget = read_journal(&a.data).map(|j| j.forget).unwrap_or_default();
    let moved = a.dir.join(MOVED_STORE);
    if moved.exists() {
        remove_dir_retry(&a.store).map_err(|e| format!("couldn't remove the half-made store ({e})"))?;
        std::fs::rename(&moved, &a.store).map_err(|e| format!("couldn't put the saved store back ({e})"))?;
        if let Some(p) = a.store.parent() {
            sync_dir(p).map_err(|e| format!("couldn't make the saved store's return durable ({e})"))?;
        }
    }
    // a fresh marker (M10) is about to be overwritten: its key id goes into the journal first
    let fresh_key = marker_id(&a.data.join(KEY_MARKER));
    let copy_key = marker_id(&a.dir.join(SIDE_DIR).join(KEY_MARKER));
    if let Some(k) = fresh_key.filter(|k| copy_key.as_deref() != Some(k.as_str()) && !forget.contains(k)) {
        forget.push(k);
        write_journal(a, "pending", &forget).map_err(|e| format!("couldn't note a key to forget ({e})"))?;
    }
    for f in SIDE_FILES {
        let here = a.data.join(f);
        if a.side.iter().any(|s| s == f) {
            let bytes = std::fs::read(a.dir.join(SIDE_DIR).join(f)).map_err(|e| format!("couldn't read the copy of {f} ({e})"))?;
            write_durable(&here, &bytes).map_err(|e| format!("couldn't put {f} back ({e})"))?;
        } else if here.exists() {
            std::fs::remove_file(&here).map_err(|e| format!("couldn't remove the new {f} ({e})"))?;
        }
    }
    // everything is back: say so durably BEFORE the copies go (a crash in the clean-up then only finishes it)
    write_journal(a, "restored", &forget).map_err(|e| format!("couldn't record the rollback ({e})"))?;
    restored_cleanup(a)?;
    eprintln!("[login] the sign-in didn't finish: the saved session is back as it was");
    Ok(forget)
}

/// After a rollback is recorded: the aside copy goes, then the journal.
fn restored_cleanup(a: &Aside) -> Result<(), String> {
    remove_dir_retry(&a.dir).map_err(|e| format!("couldn't tidy the aside copy ({e})"))?;
    std::fs::remove_file(journal_path(&a.data)).map_err(|e| format!("couldn't remove the sign-in journal ({e})"))?;
    sync_dir(&a.data).map_err(|e| format!("couldn't make the clean-up durable ({e})"))?;
    Ok(())
}

/// commit, after the new session was saved. Err = the commit couldn't be recorded (the caller rolls back).
/// Ok = the OLD store's keyring id (M10) when the new store doesn't use the same one, for the caller to forget.
pub fn commit(a: &Aside) -> Result<Option<String>, String> {
    let r = commit_inner(a);
    if r.is_ok() {
        set_active(&a.data, false);
    }
    r
}

fn commit_inner(a: &Aside) -> Result<Option<String>, String> {
    if let Err(e) = write_journal(a, "done", &[]) {
        // the journal may or may not say "done" now: before anyone rolls back, it must durably say "pending" again
        // (a "done" left behind would let the next start delete the aside copy = the saved store)
        return match write_journal(a, "pending", &[]) {
            Ok(()) => Err(format!("couldn't record the sign-in ({e})")),
            Err(e2) => {
                // can't tell the journal anything: keep the new session and leave the aside copy for the next start,
                // which resolves either state consistently ("done" keeps the new one, "pending" restores the old one)
                eprintln!("[login] the sign-in couldn't be recorded ({e}; {e2}); the next start finishes it");
                set_active(&a.data, false);
                Ok(None)
            }
        };
    }
    Ok(finish_cleanup(a))
}

fn finish_cleanup(a: &Aside) -> Option<String> {
    let old_key = marker_id(&a.dir.join(SIDE_DIR).join(KEY_MARKER));
    let new_key = marker_id(&a.data.join(KEY_MARKER));
    match remove_dir_retry(&a.dir) {
        Ok(()) => {
            let _ = std::fs::remove_file(journal_path(&a.data));
            let _ = sync_dir(&a.data);
            eprintln!("[login] signed in: the replaced store copy is gone");
        }
        // the journal says "done": the next start finishes it
        Err(e) => eprintln!("[login] the old store copy {} couldn't be removed yet ({e}); the next start finishes it", a.dir.display()),
    }
    old_key.filter(|o| new_key.as_deref() != Some(o.as_str()))
}

/// What [`recover_on_start`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recovered {
    Nothing,
    /// a sign-in a crash interrupted was undone (fresh keyring ids to forget)
    PutBack(Vec<String>),
    /// a finished sign-in's old copy was removed (its keyring id, if the new store doesn't share it, to forget)
    Removed(Option<String>),
    /// a leftover `*.replaced-*` with no journal: left alone, never guessed (logged)
    LeftAlone(PathBuf),
    /// the journal couldn't be resolved: nothing may open the store (words for the card)
    Failed(String),
    /// a sign-in is running in this process: nothing may open the store now
    Busy,
}

fn leftovers(store: &Path) -> Vec<PathBuf> {
    let (Some(parent), Some(name)) = (store.parent(), store.file_name()) else { return vec![] };
    let prefix = format!("{}.replaced-", name.to_string_lossy());
    let mut v: Vec<PathBuf> = std::fs::read_dir(parent)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&prefix))).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// At start, before the saved session is restored: finish or undo a sign-in a crash interrupted. Does nothing while a
/// sign-in runs in this process.
pub fn recover_on_start(data: &Path, store: &Path) -> Recovered {
    if is_active(data) {
        return Recovered::Busy;
    }
    recover(data, store)
}

fn recover(data: &Path, store: &Path) -> Recovered {
    let j = match journal(data) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("[login] start: {e}");
            return Recovered::Failed(e);
        }
    };
    if let Some(j) = j {
        let a = Aside { dir: store.with_file_name(&j.aside), data: data.to_path_buf(), store: store.to_path_buf(), side: j.side.clone() };
        if !a.dir.exists() {
            // prepare never finished (or the clean-up did): the live store and files are the real ones
            let _ = std::fs::remove_file(journal_path(data));
            eprintln!("[login] start: a sign-in journal named {} but it is gone; journal removed", j.aside);
            return if j.forget.is_empty() { Recovered::Nothing } else { Recovered::PutBack(j.forget) };
        }
        if j.state == "restored" {
            return match restored_cleanup(&a) {
                Ok(()) => {
                    eprintln!("[login] start: finished an interrupted rollback's clean-up ({})", a.dir.display());
                    Recovered::PutBack(j.forget)
                }
                Err(e) => Recovered::Failed(e),
            };
        }
        if j.state == "done" {
            let k = finish_cleanup(&a);
            eprintln!("[login] start: finished an interrupted sign-in's clean-up ({})", a.dir.display());
            return if read_journal(data).is_some() { Recovered::Failed("the old copy couldn't be removed".into()) } else { Recovered::Removed(k) };
        }
        return match put_back(&a) {
            Ok(k) => {
                eprintln!("[login] start: a sign-in was interrupted; the saved session is back ({})", a.dir.display());
                Recovered::PutBack(k)
            }
            Err(e) => {
                eprintln!("[login] start: putting the saved session back failed: {e}");
                Recovered::Failed(e)
            }
        };
    }
    let Some(l) = leftovers(store).last().cloned() else { return Recovered::Nothing };
    if store.exists() {
        eprintln!("[login] start: a leftover {} next to a live store and no journal: left alone", l.display());
        return Recovered::LeftAlone(l);
    }
    if !l.join(MOVED_STORE).is_dir() {
        // a preparation that never got as far as the journal (only side-file copies, no store moved): it is not a
        // saved session, and putting its session.json back could bring an account back that was signed out since
        // (assembly-038 review)
        eprintln!("[login] start: a leftover {} without a store and no journal: left alone", l.display());
        return Recovered::LeftAlone(l);
    }
    // no journal (lost) and no live store: the leftover is the saved session — put it back; the side files that
    // existed are exactly the copies it holds
    let side = SIDE_FILES.iter().filter(|f| l.join(SIDE_DIR).join(f).exists()).map(|f| f.to_string()).collect();
    let a = Aside { dir: l.clone(), data: data.to_path_buf(), store: store.to_path_buf(), side };
    match put_back(&a) {
        Ok(k) => {
            eprintln!("[login] start: no live store and a leftover {}: put back", l.display());
            Recovered::PutBack(k)
        }
        Err(e) => {
            eprintln!("[login] start: putting {} back failed: {e}", l.display());
            Recovered::Failed(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> (PathBuf, PathBuf) {
        let d = std::env::temp_dir().join(format!("login-swap-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("store")).unwrap();
        (d.clone(), d.join("store"))
    }
    fn saved(data: &Path, store: &Path, with_marker: bool) {
        std::fs::write(store.join("matrix-sdk-crypto.sqlite3"), b"OLD-CRYPTO").unwrap();
        std::fs::write(store.join("matrix-sdk-crypto.sqlite3-wal"), b"OLD-WAL").unwrap();
        std::fs::write(data.join("session.json"), b"OLD-SESSION").unwrap();
        std::fs::write(data.join("store-writer.json"), br#"{"v":1,"app":"12.1.1"}"#).unwrap();
        if with_marker {
            std::fs::write(data.join("store-key.json"), br#"{"v":1,"id":"oldkey"}"#).unwrap();
        }
    }
    fn intact(data: &Path, store: &Path, with_marker: bool) {
        assert_eq!(std::fs::read(store.join("matrix-sdk-crypto.sqlite3")).unwrap(), b"OLD-CRYPTO");
        assert_eq!(std::fs::read(store.join("matrix-sdk-crypto.sqlite3-wal")).unwrap(), b"OLD-WAL");
        assert_eq!(std::fs::read_dir(store).unwrap().count(), 2, "nothing else in the store");
        assert_eq!(std::fs::read(data.join("session.json")).unwrap(), b"OLD-SESSION");
        assert!(data.join("store-writer.json").exists());
        assert_eq!(marker_id(&data.join("store-key.json")).as_deref(), if with_marker { Some("oldkey") } else { None });
        assert!(!journal_path(data).exists(), "journal gone");
        assert!(leftovers(store).is_empty(), "no aside copy left");
    }
    /// what a sign-in leaves behind before it fails / crashes: a fresh store, a new key marker, a new session
    fn fresh(data: &Path, store: &Path, key: Option<&str>, session: bool) {
        std::fs::create_dir_all(store).unwrap();
        std::fs::write(store.join("matrix-sdk-crypto.sqlite3"), b"NEW").unwrap();
        if let Some(k) = key {
            std::fs::write(data.join("store-key.json"), format!(r#"{{"v":1,"id":"{k}"}}"#)).unwrap();
        }
        if session {
            std::fs::write(data.join("session.json"), b"NEW-SESSION").unwrap();
        }
    }
    fn aside(b: Begin) -> Aside {
        match b {
            Begin::Aside(a, _) => a,
            Begin::Fresh { .. } => panic!("expected an aside"),
        }
    }

    #[test]
    fn a_failed_sign_in_puts_everything_back_exactly() {
        for m in [false, true] {
            let (d, s) = tmp(&format!("fail{m}"));
            saved(&d, &s, m);
            let a = aside(begin(&d, &s).unwrap());
            assert!(!s.exists() && a.dir.join(MOVED_STORE).exists() && journal_path(&d).exists());
            assert!(!d.join("store-key.json").exists(), "the old marker must not reach the fresh store");
            fresh(&d, &s, if m { Some("newkey") } else { None }, false);
            assert_eq!(put_back(&a).unwrap(), if m { vec!["newkey".to_string()] } else { vec![] });
            intact(&d, &s, m);
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    #[test]
    fn a_new_session_written_before_a_failed_commit_is_undone_too() {
        let (d, s) = tmp("newsession");
        saved(&d, &s, true);
        let a = aside(begin(&d, &s).unwrap());
        fresh(&d, &s, Some("newkey"), true); // signed in, session saved, then the commit failed
        put_back(&a).unwrap();
        intact(&d, &s, true);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_committed_sign_in_removes_the_old_copy_and_names_its_key() {
        let (d, s) = tmp("ok");
        saved(&d, &s, true);
        let a = aside(begin(&d, &s).unwrap());
        fresh(&d, &s, Some("newkey"), true);
        assert_eq!(commit(&a).unwrap().as_deref(), Some("oldkey"));
        assert!(!a.dir.exists() && !journal_path(&d).exists() && s.exists());
        assert_eq!(std::fs::read(d.join("session.json")).unwrap(), b"NEW-SESSION");
        // the same key reused → nothing to forget; no old key → nothing
        let (d2, s2) = tmp("ok2");
        saved(&d2, &s2, true);
        let a2 = aside(begin(&d2, &s2).unwrap());
        fresh(&d2, &s2, Some("oldkey"), true);
        assert_eq!(commit(&a2).unwrap(), None);
        let (d3, s3) = tmp("ok3");
        saved(&d3, &s3, false);
        let a3 = aside(begin(&d3, &s3).unwrap());
        fresh(&d3, &s3, None, true);
        assert_eq!(commit(&a3).unwrap(), None);
        for x in [d, d2, d3] {
            let _ = std::fs::remove_dir_all(&x);
        }
    }

    #[test]
    fn no_store_nothing_to_protect_and_a_stray_marker_is_named() {
        let (d, s) = tmp("none");
        std::fs::remove_dir_all(&s).unwrap();
        std::fs::write(d.join("store-key.json"), br#"{"v":1,"id":"stray"}"#).unwrap();
        match begin(&d, &s).unwrap() {
            Begin::Fresh { stray_keys } => assert_eq!(stray_keys, vec!["stray".to_string()]),
            Begin::Aside(..) => panic!(),
        }
        assert!(!journal_path(&d).exists() && !d.join("store-key.json").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_crash_at_any_step_is_undone_at_start() {
        // (a) right after the move (no fresh store yet)
        let (d, s) = tmp("crash-a");
        saved(&d, &s, true);
        let _ = aside(begin(&d, &s).unwrap());
        set_active(&d, false); // a new process
        assert_eq!(recover_on_start(&d, &s), Recovered::PutBack(vec![]));
        intact(&d, &s, true);
        // (b) with a half-made fresh store, its key and even the new session saved (crash before the commit)
        let (d2, s2) = tmp("crash-b");
        saved(&d2, &s2, true);
        let _ = aside(begin(&d2, &s2).unwrap());
        fresh(&d2, &s2, Some("newkey"), true);
        set_active(&d2, false); // a new process
        assert_eq!(recover_on_start(&d2, &s2), Recovered::PutBack(vec!["newkey".into()]));
        intact(&d2, &s2, true);
        // (c) during preparation: the journal is written but the store hasn't moved yet
        let (d3, s3) = tmp("crash-c");
        saved(&d3, &s3, false);
        let a3 = aside(begin(&d3, &s3).unwrap());
        std::fs::rename(a3.dir.join(MOVED_STORE), &s3).unwrap(); // = the rename never happened
        set_active(&d3, false); // a new process
        assert_eq!(recover_on_start(&d3, &s3), Recovered::PutBack(vec![]));
        intact(&d3, &s3, false);
        // (d) a rollback that stopped half-way runs again safely: store back, side files not yet
        let (d4, s4) = tmp("crash-d");
        saved(&d4, &s4, true);
        let a4 = aside(begin(&d4, &s4).unwrap());
        fresh(&d4, &s4, Some("newkey"), true);
        std::fs::remove_dir_all(&s4).unwrap();
        std::fs::rename(a4.dir.join(MOVED_STORE), &s4).unwrap(); // half of put_back done
        set_active(&d4, false); // a new process
        assert_eq!(recover_on_start(&d4, &s4), Recovered::PutBack(vec!["newkey".into()]));
        intact(&d4, &s4, true);
        // a clean start does nothing, twice
        assert_eq!(recover_on_start(&d4, &s4), Recovered::Nothing);
        for x in [d, d2, d3, d4] {
            let _ = std::fs::remove_dir_all(&x);
        }
    }

    #[test]
    fn a_crash_after_the_commit_finishes_the_clean_up_and_a_lost_journal_is_handled() {
        let (d, s) = tmp("done");
        saved(&d, &s, true);
        let a = aside(begin(&d, &s).unwrap());
        fresh(&d, &s, Some("newkey"), true);
        write_journal(&a, "done", &[]).unwrap(); // committed, crashed before the clean-up
        set_active(&d, false); // a new process
        assert_eq!(recover_on_start(&d, &s), Recovered::Removed(Some("oldkey".into())));
        assert!(!a.dir.exists() && s.exists() && !journal_path(&d).exists());
        assert_eq!(std::fs::read(d.join("session.json")).unwrap(), b"NEW-SESSION", "the new session stays");
        // no journal (lost) + a leftover copy + no live store: put back automatically
        let (d2, s2) = tmp("stray");
        saved(&d2, &s2, true);
        let _a2 = aside(begin(&d2, &s2).unwrap());
        std::fs::remove_file(journal_path(&d2)).unwrap();
        set_active(&d2, false); // a new process
        assert_eq!(recover_on_start(&d2, &s2), Recovered::PutBack(vec![]));
        intact(&d2, &s2, true);
        // no journal + a leftover copy NEXT TO a live store: never guessed
        let (d3, s3) = tmp("stray-live");
        saved(&d3, &s3, false);
        let a3 = aside(begin(&d3, &s3).unwrap());
        std::fs::remove_file(journal_path(&d3)).unwrap();
        fresh(&d3, &s3, None, true);
        set_active(&d3, false); // a new process
        assert_eq!(recover_on_start(&d3, &s3), Recovered::LeftAlone(a3.dir.clone()));
        assert!(a3.dir.exists() && s3.exists());
        // no journal + a half-made preparation (side-file copies, no store moved), then a sign-out took the live
        // store: never put back (it would bring the signed-out session back; assembly-038 review)
        let (d4, s4) = tmp("half-prep");
        saved(&d4, &s4, false);
        let half = s4.with_file_name("store.replaced-1");
        std::fs::create_dir_all(half.join(SIDE_DIR)).unwrap();
        std::fs::copy(d4.join("session.json"), half.join(SIDE_DIR).join("session.json")).unwrap();
        std::fs::remove_dir_all(&s4).unwrap();
        std::fs::remove_file(d4.join("session.json")).unwrap();
        assert_eq!(recover_on_start(&d4, &s4), Recovered::LeftAlone(half.clone()));
        assert!(!d4.join("session.json").exists() && !s4.exists(), "nothing came back");
        for x in [d, d2, d3, d4] {
            let _ = std::fs::remove_dir_all(&x);
        }
    }

    #[test]
    fn a_pending_sign_in_is_resolved_before_the_next_one_and_recovery_waits_for_one_in_flight() {
        let (d, s) = tmp("next");
        saved(&d, &s, true);
        let _a = aside(begin(&d, &s).unwrap());
        fresh(&d, &s, Some("newkey"), false);
        // the same process: a start-up recovery while this sign-in is in flight does nothing (and says so), and a
        // second sign-in is refused
        assert_eq!(recover_on_start(&d, &s), Recovered::Busy);
        assert!(journal_path(&d).exists());
        assert!(begin(&d, &s).is_err(), "one sign-in at a time");
        // the next sign-in (a new process) first undoes the pending one, then sets the saved store aside again
        set_active(&d, false);
        let b = aside(begin(&d, &s).unwrap());
        put_back(&b).unwrap();
        intact(&d, &s, true);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_crash_in_the_rollback_clean_up_only_finishes_it_and_keeps_the_keys_to_forget() {
        let (d, s) = tmp("restored");
        saved(&d, &s, true);
        let a = aside(begin(&d, &s).unwrap());
        fresh(&d, &s, Some("newkey"), true);
        // the rollback restored everything and recorded it, then crashed while deleting the copies
        let mut put = a.clone();
        put.side = a.side.clone();
        std::fs::remove_dir_all(&s).unwrap();
        std::fs::rename(a.dir.join(MOVED_STORE), &s).unwrap();
        for f in SIDE_FILES {
            let c = a.dir.join(SIDE_DIR).join(f);
            if c.exists() {
                std::fs::copy(&c, d.join(f)).unwrap();
            }
        }
        write_journal(&put, "restored", &["newkey".to_string()]).unwrap();
        std::fs::remove_file(a.dir.join(SIDE_DIR).join("session.json")).unwrap(); // half-deleted copies
        set_active(&d, false); // a new process
        assert_eq!(recover_on_start(&d, &s), Recovered::PutBack(vec!["newkey".into()]));
        intact(&d, &s, true);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_damaged_journal_blocks_the_restore_and_the_next_sign_in() {
        let (d, s) = tmp("damaged");
        saved(&d, &s, false);
        std::fs::write(journal_path(&d), b"{not json").unwrap();
        assert!(matches!(recover_on_start(&d, &s), Recovered::Failed(_)));
        assert!(begin(&d, &s).is_err());
        assert_eq!(std::fs::read(d.join("session.json")).unwrap(), b"OLD-SESSION", "nothing touched");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn a_commit_that_cannot_be_recorded_never_leaves_a_done_that_deletes_the_saved_store() {
        use std::os::unix::fs::PermissionsExt;
        let (d, s) = tmp("commitfail");
        saved(&d, &s, true);
        let a = aside(begin(&d, &s).unwrap());
        fresh(&d, &s, Some("newkey"), true);
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o555)).unwrap(); // the data dir refuses writes
        let r = commit(&a);
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
        if std::fs::metadata(&d).map(|m| m.permissions().readonly()).unwrap_or(false) || r.is_err() {
            // (as root the write succeeds anyway; then this case can't be produced here)
        }
        // whatever the commit said, the journal never says "done" while the aside copy is the only saved store
        let j = read_journal(&d);
        assert!(j.as_ref().map_or(true, |j| j.state != "done") || !a.dir.exists(), "no 'done' next to a live aside copy");
        // and the next start resolves it consistently: the saved session back, nothing lost
        set_active(&d, false);
        if a.dir.exists() {
            assert!(matches!(recover_on_start(&d, &s), Recovered::PutBack(_)));
            intact(&d, &s, true);
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
