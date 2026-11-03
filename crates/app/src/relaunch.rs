//! 12.1.8: a relaunch (Restart now, the update banner's restart) quits this instance ONLY once the new one is known
//! to be running. Before, a failed relaunch quit anyway: the Flatpak's `flatpak-spawn --latest-version` was fired and
//! forgotten (a portal error, a missing portal or a child that died at once still ended in `exit(0)`), and Windows
//! ignored a failed spawn. The person was left with no Tin Hat at all.
//!
//! The rules (pure parts unit-tested below; update.rs and flatpak.rs apply them):
//! - **Flatpak**: the portal owns the child, so "spawned" proves nothing, and neither does a running `flatpak-spawn`
//!   (it learns of the child's exit asynchronously). The child proves it runs itself: first thing in `main()`, before
//!   it waits for our single-instance lock, it creates the ack file we named (`TINHAT_RELAUNCH_ACK`, a random name in
//!   `$XDG_CACHE_HOME/tinhat-relaunch/`, which every instance of the app sees at the same place), takes an exclusive
//!   `flock` on it, writes the name's token into it and keeps the lock for its whole life. "Running" = the token is
//!   there AND the file is locked. We quit only on Running while `flatpak-spawn` still runs, and we look again right
//!   before `exit` (the Qt thread may run the quit late: a child that gave up waiting in between is not handed a dead
//!   restart). Otherwise — the helper exited, the child's lock is gone, nothing within [`CONFIRM_TIMEOUT`] — it
//!   failed: we delete the ack file (the waiting child reads that as "called off" and exits quietly), stop the
//!   helper, stay up and say so.
//! - **A binary on Linux**: `exec()` replaces this process; it returns only when it failed, so a return = failed,
//!   stay up. (The platform guarantees the rest.)
//! - **Windows**: a successful `CreateProcess` (spawn `Ok`) is the guarantee; a spawn `Err` = failed, stay up.
//!
//! On a failure the app keeps running, logs `[relaunch] FAILED: <reason> — still running` and shows [`FAILED_MESSAGE`].

use std::time::{Duration, Instant};

/// What the person reads when a restart did not happen (Main.qml's banner shows it).
pub const FAILED_MESSAGE: &str = "Couldn't restart Tin Hat — it's still running; restart it yourself to apply the change.";

/// How long the Flatpak path waits for the new instance to say it runs. A cold start of a new deployment
/// (bwrap + the dynamic loader) is about a second on the gate machines; this is the slow-disk margin.
pub const CONFIRM_TIMEOUT: Duration = Duration::from_secs(20);

/// The env var naming the file the relaunched child creates as soon as it runs.
pub const ACK_ENV: &str = "TINHAT_RELAUNCH_ACK";

/// What the ack file says about the child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ack {
    /// no file (yet)
    Absent,
    /// the file is there but the token isn't (the child is writing it, or died doing so)
    Writing,
    /// the token is there and the child holds the file's lock: it runs
    Running,
    /// the token is there but nobody holds the lock: the child started and is gone
    Exited,
}

/// One look at a pending Flatpak relaunch.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// the new instance runs: quit now
    Started,
    /// it did not start, or not for long (why): stay up
    Failed(String),
    /// keep looking
    Pending,
}

/// The decision. `helper_exit` = flatpak-spawn's exit status if it has exited (`Some(None)` = killed by a signal).
/// Any helper exit fails the attempt even if the child looked alive: we would hand over to something we can no
/// longer stop (the child is told to stand down instead).
pub fn judge(helper_exit: Option<Option<i32>>, ack: Ack, elapsed: Duration, timeout: Duration) -> Verdict {
    if ack == Ack::Exited {
        return Verdict::Failed("the new instance started and exited".to_string());
    }
    if let Some(code) = helper_exit {
        return Verdict::Failed(match code {
            Some(c) => format!("flatpak-spawn exited with status {c} before the new instance was running"),
            None => "flatpak-spawn was killed before the new instance was running".to_string(),
        });
    }
    match ack {
        Ack::Running => Verdict::Started,
        _ if elapsed >= timeout => {
            Verdict::Failed(format!("the new instance did not report that it runs within {}s", timeout.as_secs()))
        }
        _ => Verdict::Pending,
    }
}

/// Poll [`judge`] until it decides. `helper_exit` / `ack` are the two observations; `tick` the poll period.
pub fn wait_confirmed(
    mut helper_exit: impl FnMut() -> Option<Option<i32>>,
    mut ack: impl FnMut() -> Ack,
    timeout: Duration,
    tick: Duration,
) -> Result<(), String> {
    let t0 = Instant::now();
    loop {
        let a = ack();
        let h = helper_exit();
        match judge(h, a, t0.elapsed(), timeout) {
            Verdict::Started => return Ok(()),
            Verdict::Failed(why) => return Err(why),
            Verdict::Pending => std::thread::sleep(tick),
        }
    }
}

/// A Linux binary's re-exec: `exec()` returned, so it failed (it never returns on success).
pub fn exec_returned(err: &std::io::Error) -> String {
    format!("re-exec failed: {err}")
}

/// Windows: whether the spawn result lets us quit (`Ok` = the process exists; the platform guarantees it started).
pub fn spawn_verdict<T>(r: &std::io::Result<T>) -> Result<(), String> {
    match r {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("could not start the new instance: {e}")),
    }
}

/// The one log line of a failed relaunch (the gates grep it).
pub fn log_failed(why: &str) {
    eprintln!("[relaunch] FAILED: {why} — still running");
}

/// The token in an ack file's name: 32 lowercase hex digits, then `.ack`. Anything else is not ours.
pub fn token_of(p: &std::path::Path) -> Option<String> {
    let name = p.file_name()?.to_str()?;
    let t = name.strip_suffix(".ack")?;
    (t.len() == 32 && t.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))).then(|| t.to_string())
}

/// This instance's ack folder: `$XDG_CACHE_HOME/tinhat-relaunch` (inside the sandbox `~/.var/app/<id>/cache/…`, the
/// same for every instance of the app), else `$HOME/.cache/tinhat-relaunch`.
pub fn ack_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")).filter(|p| p.is_absolute()))
        .map(|b| b.join("tinhat-relaunch"))
}

#[cfg(unix)]
mod files {
    use super::{token_of, Ack};
    use std::io::{Read, Write};
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::os::unix::io::AsRawFd;
    use std::path::{Path, PathBuf};

    /// `dir` exists as a real folder (not a symlink), owned by us, 0700 — created if missing.
    pub fn private_dir(dir: &Path) -> Result<(), String> {
        match std::fs::symlink_metadata(dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            }
            Err(e) => return Err(format!("cannot read {}: {e}", dir.display())),
            Ok(_) => {}
        }
        let m = std::fs::symlink_metadata(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        use std::os::unix::fs::MetadataExt;
        if !m.file_type().is_dir() || m.uid() != unsafe { libc::getuid() } {
            return Err(format!("{} is not our own folder", dir.display()));
        }
        if m.permissions().mode() & 0o077 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| format!("cannot make {} private: {e}", dir.display()))?;
        }
        Ok(())
    }

    /// The parent: a fresh ack path with a random token (128 bits) in `dir`; stale files (nobody holds their lock)
    /// are swept first.
    pub fn new_ack_path(dir: &Path) -> Result<PathBuf, String> {
        private_dir(dir)?;
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if let Some(t) = token_of(&p) {
                    if matches!(ack_state(&p, &t), Ack::Exited | Ack::Writing) && !locked(&p) {
                        let _ = std::fs::remove_file(&p);
                    }
                }
            }
        }
        let token = format!("{:032x}", rand::random::<u128>());
        let p = dir.join(format!("{token}.ack"));
        match std::fs::symlink_metadata(&p) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(p),
            _ => Err(format!("{} already exists", p.display())),
        }
    }

    fn open_nofollow(p: &Path) -> std::io::Result<std::fs::File> {
        std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(p)
    }

    /// Whether someone holds the file's lock (a regular file only).
    fn locked(p: &Path) -> bool {
        let Ok(f) = open_nofollow(p) else { return false };
        if !f.metadata().map(|m| m.is_file()).unwrap_or(false) {
            return false;
        }
        let r = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
        if r == 0 {
            unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_UN) };
            false
        } else {
            true // contended, or the probe failed: keep the file (never sweep what we cannot read)
        }
    }

    /// The parent's look at the ack file (never follows a symlink, never blocks on a FIFO).
    pub fn ack_state(p: &Path, token: &str) -> Ack {
        let Ok(f) = open_nofollow(p) else { return Ack::Absent };
        if !f.metadata().map(|m| m.is_file()).unwrap_or(false) {
            return Ack::Writing; // not a regular file: never Running (times out)
        }
        let mut s = String::new();
        if Read::take(&f, 256).read_to_string(&mut s).is_err() || s.trim() != token {
            return Ack::Writing;
        }
        let r = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
        if r == 0 {
            unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_UN) };
            return Ack::Exited;
        }
        // only contention proves the child holds it; any other error (ENOLCK, …) proves nothing: never Running
        if std::io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK) {
            Ack::Running
        } else {
            Ack::Writing
        }
    }

    /// The child: create `p` (must be `<dir>/<token>.ack`, a new regular file — never through a symlink), lock it,
    /// write the token. The returned file must stay open for the process's life (the lock is the liveness proof).
    pub fn child_ack_at(dir: &Path, p: &Path) -> Result<std::fs::File, String> {
        let token = token_of(p).ok_or_else(|| "not an ack file name".to_string())?;
        if p.parent() != Some(dir) {
            return Err(format!("not in {}", dir.display()));
        }
        let m = std::fs::symlink_metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        if !m.file_type().is_dir() {
            return Err(format!("{} is not a folder", dir.display()));
        }
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(p)
            .map_err(|e| format!("cannot create {}: {e}", p.display()))?;
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("cannot lock the ack file".to_string());
        }
        f.write_all(format!("{token}\n").as_bytes()).map_err(|e| format!("cannot write the ack file: {e}"))?;
        Ok(f)
    }
}
#[cfg(unix)]
pub use files::{ack_state, child_ack_at, new_ack_path};

/// The child's ack (path + the locked file), kept for the process's life.
static CHILD_ACK: std::sync::Mutex<Option<(std::path::PathBuf, std::fs::File)>> = std::sync::Mutex::new(None);

/// The child side, first thing in `main()` (before the single-instance lock, which the parent holds until it has
/// seen this): create + lock the file the parent named, drop the variable so a later relaunch of ours never reuses it.
pub fn ack_if_asked() {
    let Some(p) = std::env::var_os(ACK_ENV) else { return };
    std::env::remove_var(ACK_ENV);
    #[cfg(unix)]
    {
        let p = std::path::PathBuf::from(p);
        let Some(dir) = ack_dir() else {
            eprintln!("[relaunch] no cache folder: cannot tell the previous instance that we run");
            return;
        };
        match child_ack_at(&dir, &p) {
            Ok(f) => {
                eprintln!("[relaunch] started; told the previous instance");
                *CHILD_ACK.lock().unwrap() = Some((p, f));
            }
            Err(e) => eprintln!("[relaunch] could not tell the previous instance that we run ({e})"),
        }
    }
    #[cfg(not(unix))]
    let _ = p;
}

/// 12.1.8 (assembly-044 review): what the previous instance does once the child runs. The app stays usable while it
/// waits (up to [`CONFIRM_TIMEOUT`]), so a call may have started: a hand-over never ends a call.
#[derive(Debug, PartialEq, Eq)]
pub enum HandOver {
    Commit,
    WaitForCall,
}
pub fn at_hand_over(call_up: bool) -> HandOver {
    if call_up { HandOver::WaitForCall } else { HandOver::Commit }
}

/// The child gives up waiting: withdraw the hand-over (delete the file, drop the lock) so the previous instance can
/// no longer commit to us. `true` = there was one.
pub fn withdraw() -> bool {
    match CHILD_ACK.lock().unwrap().take() {
        Some((p, f)) => {
            let _ = std::fs::remove_file(&p);
            drop(f);
            eprintln!("[relaunch] gave up waiting for the previous instance — hand-over withdrawn");
            true
        }
        None => false,
    }
}

/// The child, while it waits for the previous instance's lock: the previous instance deleted our ack file, i.e. it
/// called the restart off and stays up — we exit quietly instead of reporting "already running".
pub fn called_off() -> bool {
    match &*CHILD_ACK.lock().unwrap() {
        Some((p, _)) => std::fs::symlink_metadata(p).is_err(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const T: Duration = Duration::from_secs(20);

    #[test]
    fn a_running_child_while_the_helper_runs_is_started() {
        assert_eq!(judge(None, Ack::Running, Duration::ZERO, T), Verdict::Started);
    }
    #[test]
    fn a_helper_that_exited_is_failed_even_with_a_running_child() {
        assert!(matches!(judge(Some(Some(1)), Ack::Absent, Duration::ZERO, T), Verdict::Failed(w) if w.contains("status 1")));
        assert!(matches!(judge(Some(Some(0)), Ack::Running, Duration::ZERO, T), Verdict::Failed(_)));
        assert!(matches!(judge(Some(None), Ack::Absent, Duration::ZERO, T), Verdict::Failed(w) if w.contains("killed")));
    }
    #[test]
    fn a_child_that_started_and_exited_is_failed_while_the_helper_still_looks_alive() {
        // the helper learns of the exit asynchronously (or is stopped): the lock, not the helper, decides
        assert!(matches!(judge(None, Ack::Exited, Duration::ZERO, T), Verdict::Failed(w) if w.contains("started and exited")));
    }
    #[test]
    fn a_half_written_ack_is_not_started_and_times_out() {
        assert_eq!(judge(None, Ack::Writing, Duration::from_secs(5), T), Verdict::Pending);
        assert!(matches!(judge(None, Ack::Writing, T, T), Verdict::Failed(_)));
        assert!(matches!(judge(None, Ack::Absent, T, T), Verdict::Failed(w) if w.contains("within 20s")));
    }
    #[test]
    fn wait_confirmed_quits_only_on_a_running_child() {
        let n = Cell::new(0);
        let r = wait_confirmed(
            || None,
            || {
                n.set(n.get() + 1);
                match n.get() {
                    1 => Ack::Absent,
                    2 => Ack::Writing,
                    _ => Ack::Running,
                }
            },
            T,
            Duration::from_millis(1),
        );
        assert_eq!(r, Ok(()));
        assert_eq!(n.get(), 3);
    }
    #[test]
    fn wait_confirmed_fails_when_the_portal_refuses() {
        // flatpak-spawn's "Portal call failed": it exits 1 at once, nothing was started
        assert!(wait_confirmed(|| Some(Some(1)), || Ack::Absent, T, Duration::from_millis(1)).is_err());
    }
    #[test]
    fn wait_confirmed_times_out() {
        let r = wait_confirmed(|| None, || Ack::Absent, Duration::from_millis(30), Duration::from_millis(5));
        assert!(r.unwrap_err().contains("did not report"));
    }
    #[test]
    fn exec_and_spawn_failures_keep_running() {
        let e = std::io::Error::from_raw_os_error(8); // ENOEXEC
        assert!(exec_returned(&e).starts_with("re-exec failed"));
        let bad: std::io::Result<()> = Err(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"));
        assert!(spawn_verdict(&bad).is_err());
        assert_eq!(spawn_verdict(&Ok::<(), std::io::Error>(())), Ok(()));
    }
    #[test]
    fn ack_tokens_are_32_hex_digits() {
        let t = "0123456789abcdef0123456789abcdef";
        assert_eq!(token_of(std::path::Path::new(&format!("/x/{t}.ack"))).as_deref(), Some(t));
        for bad in ["/x/123-456.ack", "/x/0123456789ABCDEF0123456789abcdef.ack", "/x/0123456789abcdef0123456789abcdef", "/x/../.ack"] {
            assert_eq!(token_of(std::path::Path::new(bad)), None, "{bad}");
        }
    }
    #[test]
    fn the_message_says_it_is_still_running() {
        assert!(FAILED_MESSAGE.contains("still running"));
        // assembly-044: a call that started during the hand-over is never ended by it
        assert_eq!(at_hand_over(true), HandOver::WaitForCall);
        assert_eq!(at_hand_over(false), HandOver::Commit);
        assert!(FAILED_MESSAGE.contains("restart it yourself"));
    }

    #[cfg(unix)]
    mod handshake {
        use super::super::*;
        use std::os::unix::fs::symlink;

        fn tmp(name: &str) -> std::path::PathBuf {
            let d = std::env::temp_dir().join(format!("relaunch-test-{name}-{}-{:x}", std::process::id(), rand::random::<u64>()));
            std::fs::create_dir_all(&d).unwrap();
            d
        }

        #[test]
        fn the_childs_lock_is_the_proof_it_runs() {
            let d = tmp("lock").join("tinhat-relaunch");
            let p = new_ack_path(&d).unwrap();
            let t = token_of(&p).unwrap();
            assert_eq!(ack_state(&p, &t), Ack::Absent);
            let f = child_ack_at(&d, &p).unwrap();
            assert_eq!(ack_state(&p, &t), Ack::Running);
            assert_eq!(ack_state(&p, "ffffffffffffffffffffffffffffffff"), Ack::Writing, "another token is not ours");
            drop(f); // the child is gone
            assert_eq!(ack_state(&p, &t), Ack::Exited);
            // the next relaunch sweeps the stale file
            let _q = new_ack_path(&d).unwrap();
            assert!(std::fs::symlink_metadata(&p).is_err());
            let _ = std::fs::remove_dir_all(d.parent().unwrap());
        }
        #[test]
        fn the_child_never_writes_through_a_symlink_or_outside_its_folder() {
            let base = tmp("sym");
            let d = base.join("tinhat-relaunch");
            std::fs::create_dir_all(&d).unwrap();
            let victim = base.join("victim.txt");
            std::fs::write(&victim, "keep me").unwrap();
            let p = d.join("0123456789abcdef0123456789abcdef.ack");
            symlink(&victim, &p).unwrap();
            assert!(child_ack_at(&d, &p).is_err());
            assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
            assert_eq!(ack_state(&p, "0123456789abcdef0123456789abcdef"), Ack::Absent, "a symlink is never followed");
            let elsewhere = base.join("0123456789abcdef0123456789abcde0.ack");
            assert!(child_ack_at(&d, &elsewhere).is_err());
            assert!(child_ack_at(&d, &d.join("x.ack")).is_err());
            let _ = std::fs::remove_dir_all(&base);
        }
        #[test]
        fn called_off_and_withdraw_use_the_childs_own_file() {
            // the only test touching CHILD_ACK
            let d = tmp("withdraw").join("tinhat-relaunch");
            let p = new_ack_path(&d).unwrap();
            let t = token_of(&p).unwrap();
            *CHILD_ACK.lock().unwrap() = Some((p.clone(), child_ack_at(&d, &p).unwrap()));
            assert!(!called_off());
            assert_eq!(ack_state(&p, &t), Ack::Running);
            // the parent calls it off: it deletes the file
            std::fs::remove_file(&p).unwrap();
            assert!(called_off());
            // a fresh one: the child gives up and withdraws — the parent can no longer see it running
            let p = new_ack_path(&d).unwrap();
            let t = token_of(&p).unwrap();
            *CHILD_ACK.lock().unwrap() = Some((p.clone(), child_ack_at(&d, &p).unwrap()));
            assert!(withdraw());
            assert_eq!(ack_state(&p, &t), Ack::Absent);
            assert!(!withdraw(), "only once");
            assert!(!called_off());
            let _ = std::fs::remove_dir_all(d.parent().unwrap());
        }
        #[test]
        fn a_symlinked_ack_folder_is_refused() {
            let base = tmp("dirsym");
            let real = base.join("real");
            std::fs::create_dir_all(&real).unwrap();
            let d = base.join("tinhat-relaunch");
            symlink(&real, &d).unwrap();
            assert!(new_ack_path(&d).is_err());
            assert!(child_ack_at(&d, &d.join("0123456789abcdef0123456789abcdef.ack")).is_err());
            let _ = std::fs::remove_dir_all(&base);
        }
    }
}
