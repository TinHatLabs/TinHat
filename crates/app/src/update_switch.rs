//! 12.1.14 (win-updater-safety): the Windows in-app update never leaves the shortcuts pointing at a program Windows
//! refuses to start.
//!
//! Why: the Windows builds are unsigned, and Smart App Control (SAC) judges every new unsigned file on its own; some
//! builds pass, some don't, and a DLL can be refused while the exe is allowed. Up to 12.1.13 the updater switched the
//! folders as soon as the download was done (current\ -> backup\, the new bundle -> current\). When Windows then
//! refused the new tinhat.exe, the old instance stayed up with "Couldn't restart Tin Hat…", and once the person quit,
//! every shortcut (Setup's and the in-app install's all point at current\tinhat.exe) started the refused file:
//! locked out, with the old version sitting unused in backup\.
//!
//! The rule now: **current\ only ever holds a bundle that has already started on this PC** (or the one Setup put
//! there). No code of ours runs when Windows refuses current\tinhat.exe, so nothing could repair it afterwards; the
//! only safe design is to never leave an unproven bundle there beyond one supervised start.
//!
//! 1. Download: the bundle is unzipped into staging\bundle and PROBED there: `tinhat.exe --update-probe` starts with
//!    no window, maps every PE file of its own bundle once (the exe, every DLL and plugin: a DLL Windows would refuse
//!    only when it is loaded later is caught now), prints one report and exits. No sign-in, no settings, no data.
//!    Only a clean probe writes staging\READY. Anything else: staging is deleted, current\ was never touched, and the
//!    person reads "Windows blocked this update, so Tin Hat stays on <version>. You can keep using it."
//! 2. The switch happens only at a restart (the banner's Restart, or the next normal start of the old version):
//!    current\ -> backup\, staging\bundle -> current\, start current\tinhat.exe, wait for its "I run" ack
//!    (relaunch.rs). Refused (CreateProcess fails), gone before the ack (a refused DLL kills it in the loader) or no
//!    ack within [`START_TIMEOUT`] (stopped): the folders go back (current\ = the old version), staging is deleted,
//!    the old instance stays up and says so. Started: the old instance quits; the new one sweeps backup\ at its
//!    next start.
//!
//! Left open (said in UPDATE_PATH.md §3.6): the old process dying (power loss) in the seconds between the switch and
//! the ack leaves an unproven current\ (backup\ still holds the old version). A clean probe is a strong, not a
//! perfect, predictor (the verdict comes from Microsoft's cloud); the supervised start covers the rest.
//!
//! Everything above the `win` module is plain std and unit-tested on Linux (the folder moves are real renames in a
//! temp folder); `win` is the Windows process glue.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The bundle's program name.
pub const EXE: &str = "tinhat.exe";
/// `tinhat.exe --update-probe`: the probe mode (main.rs handles it first thing).
pub const PROBE_ARG: &str = "--update-probe";
/// Every probe report line starts with this.
pub const PROBE_TAG: &str = "TINHAT-UPDATE-PROBE";
/// The probe maps a few hundred files; on a first sight Windows may ask its cloud about each one.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(180);
/// How long the old instance waits for the new one's ack after the switch.
pub const START_TIMEOUT: Duration = Duration::from_secs(30);
/// Where the help page lives (tinhat.dev; the page is "If Windows blocks Tin Hat").
pub const HELP: &str = "tinhat.dev/windows-blocked";

/// The updater's folders under the install root (`%LOCALAPPDATA%\TinHatApp`).
#[derive(Clone, Debug)]
pub struct Layout {
    pub root: PathBuf,
}

impl Layout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    /// What every shortcut starts.
    pub fn current(&self) -> PathBuf {
        self.root.join("current")
    }
    /// The previous version during and right after a switch.
    pub fn backup(&self) -> PathBuf {
        self.root.join("backup")
    }
    pub fn staging(&self) -> PathBuf {
        self.root.join("staging")
    }
    /// The downloaded bundle, waiting (always this exact folder, whatever shape the zip had).
    pub fn staged(&self) -> PathBuf {
        self.staging().join("bundle")
    }
    /// Where the zip is unpacked before its bundle folder moves to [`Self::staged`].
    pub fn unpack(&self) -> PathBuf {
        self.staging().join("unpack")
    }
    fn ready_file(&self) -> PathBuf {
        self.staging().join("READY")
    }
    /// A switched-in bundle that didn't start, on its way out.
    fn refused(&self) -> PathBuf {
        self.staging().join("refused")
    }
    /// The message for the next start when a switch at startup didn't work (the UI isn't up yet then).
    pub fn note(&self) -> PathBuf {
        self.root.join("update-blocked.txt")
    }
}

/// How hard a folder move tries (an antivirus scan can hold a file for a moment).
#[derive(Clone, Copy, Debug)]
pub struct Retry {
    pub tries: u32,
    pub pause: Duration,
}

impl Default for Retry {
    fn default() -> Self {
        Self { tries: 25, pause: Duration::from_millis(200) }
    }
}

fn rename_retry(from: &Path, to: &Path, r: &Retry) -> Result<(), String> {
    let mut last = String::new();
    for i in 0..r.tries.max(1) {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => last = e.to_string(),
        }
        if i + 1 < r.tries {
            std::thread::sleep(r.pause);
        }
    }
    Err(format!("{} -> {}: {last}", from.display(), to.display()))
}

/// The VERSION file of a bundle folder ("" when missing).
pub fn bundle_version(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("VERSION")).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// Download step, part 1: an empty staging\ with an empty staging\unpack\ to unzip into. current\ is not touched.
pub fn prepare_staging(l: &Layout) -> Result<PathBuf, String> {
    if l.staging().exists() {
        std::fs::remove_dir_all(l.staging()).map_err(|e| format!("clear staging: {e}"))?;
    }
    std::fs::create_dir_all(l.unpack()).map_err(|e| format!("mkdir staging: {e}"))?;
    Ok(l.unpack())
}

/// Download step, part 2: find the bundle in what was unzipped (tinhat.exe at the top, or in a single wrapper
/// folder), move it to staging\bundle, and check it declares `expected_version`. current\ is not touched.
pub fn adopt_unpacked(l: &Layout, expected_version: &str) -> Result<PathBuf, String> {
    let unpack = l.unpack();
    let mut bundle = unpack.clone();
    if !bundle.join(EXE).is_file() {
        let entries: Vec<_> = std::fs::read_dir(&unpack)
            .map_err(|e| format!("read staging: {e}"))?
            .filter_map(|e| e.ok())
            .collect();
        if entries.len() == 1 && entries[0].path().is_dir() && entries[0].path().join(EXE).is_file() {
            bundle = entries[0].path();
        } else {
            return Err(format!("zip has no {EXE} at top level or in a single wrapper dir"));
        }
    }
    let staged_ver = bundle_version(&bundle);
    if !expected_version.is_empty() && staged_ver != expected_version {
        return Err(format!("staged VERSION {staged_ver:?} != manifest version {expected_version:?} (bad bundle?)"));
    }
    rename_retry(&bundle, &l.staged(), &Retry::default()).map_err(|e| format!("stage the bundle: {e}"))?;
    let _ = std::fs::remove_dir_all(&unpack);
    Ok(l.staged())
}

/// Download step, part 3 (only after a clean probe): the staged bundle may be switched in at the next restart.
pub fn mark_ready(l: &Layout, version: &str) -> Result<(), String> {
    std::fs::write(l.ready_file(), version).map_err(|e| format!("mark the update ready: {e}"))
}

/// The version waiting in staging, if it's complete and probed: READY names it, the bundle has its exe, and the
/// bundle's VERSION says the same.
pub fn ready_version(l: &Layout) -> Option<String> {
    let v = std::fs::read_to_string(l.ready_file()).ok()?.trim().to_string();
    if v.is_empty() || !l.staged().join(EXE).is_file() || bundle_version(&l.staged()) != v {
        return None;
    }
    Some(v)
}

/// Throw away whatever is staged (a refused probe, a failed download). Best effort.
pub fn discard_staging(l: &Layout) {
    if l.staging().exists() {
        if let Err(e) = std::fs::remove_dir_all(l.staging()) {
            eprintln!("[update] couldn't delete {} yet ({e}); the next start tries again", l.staging().display());
        }
    }
}

/// What a start of the installed app does about the updater's folders.
#[derive(Debug, PartialEq, Eq)]
pub enum StartupPlan {
    /// A probed newer version waits in staging: switch to it, supervised (the old version stays if it won't start).
    Switch(String),
    /// Nothing waits: delete backup\ (this start proves current\ works) and any half-done staging.
    Sweep,
}

/// `newer(candidate, running)` = update.rs's is_upgrade.
pub fn startup_plan(l: &Layout, running: &str, newer: impl Fn(&str, &str) -> bool) -> StartupPlan {
    match ready_version(l) {
        Some(v) if newer(&v, running) => StartupPlan::Switch(v),
        _ => StartupPlan::Sweep,
    }
}

/// [`StartupPlan::Sweep`] at a start: only staging\ (a half-done download). backup\ stays until this start's main
/// window is on screen ([`sweep_backup`]): an ack proves main() ran, not that the app came up (Astra review).
pub fn sweep_staging(l: &Layout) {
    sweep_dirs(&[l.staging()]);
}

/// The main window is up: the previous version in backup\ is no longer needed.
pub fn sweep_backup(l: &Layout) {
    sweep_dirs(&[l.backup()]);
}

/// Both (the headless verify phase).
pub fn sweep(l: &Layout) {
    sweep_dirs(&[l.backup(), l.staging()]);
}

fn sweep_dirs(dirs: &[PathBuf]) {
    for dir in dirs {
        if dir.exists() {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => eprintln!("[platform] swept updater dir {}", dir.display()),
                Err(e) => eprintln!("[platform] sweep of {} deferred ({e}); retrying next launch", dir.display()),
            }
        }
    }
}

/// The switch: current\ -> backup\, staging\bundle -> current\. On a failed second move the first is undone.
pub fn switch(l: &Layout, r: &Retry) -> Result<(), String> {
    if !l.current().join(EXE).is_file() {
        return Err(format!("no installed app in {}", l.current().display()));
    }
    if !l.staged().join(EXE).is_file() {
        return Err("nothing staged".to_string());
    }
    if l.backup().exists() {
        std::fs::remove_dir_all(l.backup()).map_err(|e| format!("an earlier backup is still in use ({e})"))?;
    }
    rename_retry(&l.current(), &l.backup(), r).map_err(|e| format!("move the installed app aside: {e}"))?;
    if let Err(e) = rename_retry(&l.staged(), &l.current(), r) {
        return Err(match rename_retry(&l.backup(), &l.current(), r) {
            Ok(()) => format!("put the update in place: {e} (the installed app is back)"),
            Err(rb) => format!("put the update in place: {e}; MOVING THE INSTALLED APP BACK FAILED: {rb}"),
        });
    }
    Ok(())
}

/// After a switched-in version didn't start: current\ -> staging\refused, backup\ -> current\, staging deleted.
pub fn roll_back(l: &Layout, r: &Retry) -> Result<(), String> {
    if !l.backup().join(EXE).is_file() {
        return Err(format!("no previous version in {}", l.backup().display()));
    }
    let _ = std::fs::create_dir_all(l.staging());
    if l.refused().exists() {
        let _ = std::fs::remove_dir_all(l.refused());
    }
    rename_retry(&l.current(), &l.refused(), r).map_err(|e| format!("move the refused version out: {e}"))?;
    rename_retry(&l.backup(), &l.current(), r).map_err(|e| format!("put the previous version back: {e}"))?;
    discard_staging(l);
    Ok(())
}

/// A switch that started fine but was called off (a call began meanwhile): the new bundle goes back to staging (still
/// READY, so the next restart switches again) and the old one back to current\.
pub fn unswitch(l: &Layout, r: &Retry) -> Result<(), String> {
    rename_retry(&l.current(), &l.staged(), r).map_err(|e| format!("move the update back to staging: {e}"))?;
    rename_retry(&l.backup(), &l.current(), r).map_err(|e| format!("put the previous version back: {e}"))
}

/// After any failed attempt: current\ must hold the version it held before (`previous` = its VERSION then), with its
/// exe. If current\ is gone and backup\ holds a bundle, that one goes back first (Astra review: a failed second move
/// must never leave the shortcuts with nothing). Err = the shortcuts may not start the old version: say so plainly.
pub fn kept_in_place(l: &Layout, previous: &str, r: &Retry) -> Result<(), String> {
    if !l.current().exists() && l.backup().join(EXE).is_file() {
        rename_retry(&l.backup(), &l.current(), r).map_err(|e| format!("current\\ is missing and putting backup\\ back failed: {e}"))?;
    }
    let now = bundle_version(&l.current());
    if l.current().join(EXE).is_file() && now == previous {
        Ok(())
    } else if !l.current().join(EXE).is_file() {
        Err(format!("current\\ has no {EXE} after the attempt"))
    } else {
        Err(format!("current\\ holds {now:?}, not the previous {previous:?}"))
    }
}

/// Why an update did not become the running version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Windows refused it (an application-control / reputation / image-hash error).
    Blocked(String),
    /// It didn't start for another reason (crashed, no answer, broken file).
    Failed(String),
    /// The folders couldn't be moved (something holds them); nothing was started.
    NotPlaced(String),
}

impl Refusal {
    pub fn why(&self) -> &str {
        match self {
            Refusal::Blocked(w) | Refusal::Failed(w) | Refusal::NotPlaced(w) => w,
        }
    }
    /// The sentence the person reads. `kept` = the version that keeps running.
    pub fn message(&self, kept: &str) -> String {
        let kept = if kept.is_empty() { "the version you have".to_string() } else { kept.to_string() };
        match self {
            Refusal::Blocked(_) => {
                format!("Windows blocked this update, so Tin Hat stays on {kept}. You can keep using it. More: {HELP}")
            }
            Refusal::Failed(_) => {
                format!("This update didn't start on this PC, so Tin Hat stays on {kept}. You can keep using it. More: {HELP}")
            }
            Refusal::NotPlaced(_) => format!(
                "Tin Hat couldn't put the update in place (another program is using its folder), so it stays on {kept}. Try again in a minute."
            ),
        }
    }
}

/// Windows error / exit codes that mean "a policy refused this file" (Smart App Control, WDAC, a bad image hash,
/// Defender). Only the wording depends on this: every failure keeps the old version.
pub fn is_policy_code(code: i32) -> bool {
    matches!(
        code as u32,
        225 // ERROR_VIRUS_INFECTED
        | 577 // ERROR_INVALID_IMAGE_HASH
        | 1260 // ERROR_ACCESS_DISABLED_BY_POLICY
        | 4550..=4560 // ERROR_SYSTEM_INTEGRITY_* (4551 = "An Application Control policy has blocked this file")
        | 0xC000_0428 // STATUS_INVALID_IMAGE_HASH (a process the loader stopped)
        | 0xC000_0361..=0xC000_0363 | 0xC000_0372 // STATUS_ACCESS_DISABLED_BY_POLICY_*
        | 0xC000_0906 // STATUS_VIRUS_INFECTED
    )
}

fn hex(code: i32) -> String {
    format!("0x{:08X}", code as u32)
}

/// One refusal from a start that failed with OS error `code` (`None` = no code).
pub fn spawn_refusal(code: Option<i32>, what: &str) -> Refusal {
    match code {
        Some(c) if is_policy_code(c) => Refusal::Blocked(format!("Windows refused to start {what} (error {c})")),
        Some(c) => Refusal::Failed(format!("{what} could not be started (error {c})")),
        None => Refusal::Failed(format!("{what} could not be started")),
    }
}

/// The CPU a PE image is built for (`None` = not a well-formed PE header: such a file is still handed to Windows,
/// which refuses it). Real-bundle finding (a Windows build machine, 2026-11-13): the bundle ships vc_redist.x64.exe, a 32-bit
/// bootstrapper; a 64-bit process can't map it (error 193), so without this every real update read as refused.
pub fn pe_machine(head: &[u8]) -> Option<u16> {
    if head.len() < 0x40 || &head[..2] != b"MZ" {
        return None;
    }
    let off = u32::from_le_bytes(head[0x3c..0x40].try_into().ok()?) as usize;
    if head.get(off..off.checked_add(4)?)? != b"PE\0\0" {
        return None;
    }
    let m = head.get(off + 4..off + 6)?;
    Some(u16::from_le_bytes([m[0], m[1]]))
}

/// The PE machine of this process (x64 or arm64): only those images can be mapped into it.
pub const NATIVE_MACHINE: u16 = if cfg!(target_arch = "aarch64") { 0xAA64 } else { 0x8664 };

/// Whether the probe maps this file: anything but a well-formed PE for another CPU (that one is never loaded by the
/// app; it only runs as its own process, e.g. Setup's VC++ runtime installer).
pub fn probe_maps(head: &[u8]) -> bool {
    !matches!(pe_machine(head), Some(m) if m != NATIVE_MACHINE)
}

/// The probe's report (the child prints it): one `refused` line per file Windows wouldn't map, then the `ok` line.
pub fn probe_report(version: &str, files: usize, refused: &[(String, i32)]) -> String {
    let mut s = String::new();
    for (f, c) in refused {
        s.push_str(&format!("{PROBE_TAG} refused code={c} file={f}\n"));
    }
    s.push_str(&format!("{PROBE_TAG} done version={version} files={files} refused={}\n", refused.len()));
    s
}

/// The parent's verdict on a probe. `spawn_err` = the OS error when it couldn't even be started; `exit` = its exit
/// code (`None` = it didn't finish within [`PROBE_TIMEOUT`] and was stopped); `out` = what it printed. Ok = the number
/// of PE files it mapped.
pub fn judge_probe(spawn_err: Option<Option<i32>>, exit: Option<i32>, out: &str, want: &str) -> Result<usize, Refusal> {
    if let Some(code) = spawn_err {
        return Err(spawn_refusal(code, "the new version"));
    }
    let Some(exit) = exit else {
        return Err(Refusal::Failed(format!("the new version's check didn't finish within {}s", PROBE_TIMEOUT.as_secs())));
    };
    let mut refused: Vec<(String, i32)> = Vec::new();
    let mut done: Option<(String, usize)> = None;
    for line in out.lines() {
        let Some(rest) = line.trim().strip_prefix(PROBE_TAG) else { continue };
        let rest = rest.trim();
        if let Some(r) = rest.strip_prefix("refused ") {
            let code = r.split_whitespace().find_map(|w| w.strip_prefix("code=")).and_then(|c| c.parse().ok()).unwrap_or(-1);
            let file = r.split_once("file=").map(|(_, f)| f.to_string()).unwrap_or_default();
            refused.push((file, code));
        } else if let Some(r) = rest.strip_prefix("done ") {
            let ver = r.split_whitespace().find_map(|w| w.strip_prefix("version=")).unwrap_or("").to_string();
            let files = r.split_whitespace().find_map(|w| w.strip_prefix("files=")).and_then(|n| n.parse().ok()).unwrap_or(0);
            done = Some((ver, files));
        }
    }
    if let Some((file, code)) = refused.first() {
        let more = if refused.len() > 1 { format!(" (and {} more)", refused.len() - 1) } else { String::new() };
        return Err(if refused.iter().any(|(_, c)| is_policy_code(*c)) {
            Refusal::Blocked(format!("Windows refused {file} (error {code}){more}"))
        } else {
            Refusal::Failed(format!("{file} could not be loaded (error {code}){more}"))
        });
    }
    if exit != 0 {
        return Err(if is_policy_code(exit) {
            Refusal::Blocked(format!("Windows stopped the new version ({})", hex(exit)))
        } else {
            Refusal::Failed(format!("the new version's check exited with {}", hex(exit)))
        });
    }
    match done {
        Some((v, n)) if v == want && n > 0 => Ok(n),
        Some((v, _)) if v != want => Err(Refusal::Failed(format!("the check reported version {v:?}, not {want:?}"))),
        Some(_) => Err(Refusal::Failed("the check found no program files".to_string())),
        None => Err(Refusal::Failed("the new version's check didn't report".to_string())),
    }
}

/// One look at a supervised start.
#[derive(Debug, PartialEq, Eq)]
pub enum Start {
    Started,
    Pending,
    Refused(Refusal),
}

/// `exit` = the new process's exit code if it is gone; `acked` = it said it runs (relaunch ack).
pub fn judge_start(exit: Option<i32>, acked: bool, elapsed: Duration, timeout: Duration) -> Start {
    if let Some(c) = exit {
        return Start::Refused(if is_policy_code(c) {
            Refusal::Blocked(format!("Windows stopped the new version ({})", hex(c)))
        } else {
            Refusal::Failed(format!("the new version exited ({}) before it was running", hex(c)))
        });
    }
    if acked {
        return Start::Started;
    }
    if elapsed >= timeout {
        return Start::Refused(Refusal::Failed(format!(
            "the new version didn't report that it runs within {}s",
            timeout.as_secs()
        )));
    }
    Start::Pending
}

/// The started process as the supervisor sees it.
pub trait NewInstance {
    /// Its exit code once it is gone, else `None`.
    fn exit_code(&mut self) -> Option<i32>;
    /// Stop it and wait until it is gone (its files are free afterwards).
    fn stop(&mut self);
}

/// A supervised start that worked.
#[derive(Debug)]
pub struct HandedOver<C> {
    pub child: C,
    /// a staged update was switched in for it
    pub switched: bool,
    /// the version now in current\
    pub version: String,
    /// current\'s VERSION before the attempt (what a roll back must restore)
    pub previous: String,
}

/// A supervised start that didn't: the old version is in current\ again (unless `rollback` says otherwise).
#[derive(Debug)]
pub struct NotStarted {
    pub refusal: Refusal,
    /// the version that keeps running
    pub kept: String,
    /// moving the folders back (Ok when nothing was switched)
    pub rollback: Result<(), String>,
}

/// The whole restart: switch in a ready update (if one waits), start current\tinhat.exe through `spawn`, and wait for
/// `acked`. Any failure stops the new process and moves the folders back before it returns.
pub fn switch_and_start<C: NewInstance>(
    l: &Layout,
    running: &str,
    spawn: impl FnOnce(&Path) -> std::io::Result<C>,
    mut acked: impl FnMut() -> bool,
    timeout: Duration,
    tick: Duration,
    r: &Retry,
) -> Result<HandedOver<C>, NotStarted> {
    let staged = ready_version(l);
    let previous = bundle_version(&l.current());
    let switched = match &staged {
        Some(_) => match switch(l, r) {
            Ok(()) => true,
            Err(e) => {
                // a failed undo inside switch() must not be reported as "still on the old version"
                let rollback = kept_in_place(l, &previous, r);
                return Err(NotStarted { refusal: Refusal::NotPlaced(e), kept: running.to_string(), rollback });
            }
        },
        None => false,
    };
    let version = staged.unwrap_or_else(|| running.to_string());
    let give_up = |refusal: Refusal| NotStarted {
        refusal,
        kept: running.to_string(),
        rollback: if switched {
            let rb = roll_back(l, r);
            match kept_in_place(l, &previous, r) {
                Ok(()) => Ok(()),
                Err(e) => Err(match rb {
                    Ok(()) => e,
                    Err(rb) => format!("{rb}; {e}"),
                }),
            }
        } else {
            Ok(())
        },
    };
    let mut child = match spawn(&l.current().join(EXE)) {
        Ok(c) => c,
        Err(e) => return Err(give_up(spawn_refusal(e.raw_os_error(), "the new version"))),
    };
    let t0 = Instant::now();
    loop {
        match judge_start(child.exit_code(), acked(), t0.elapsed(), timeout) {
            Start::Started => return Ok(HandedOver { child, switched, version, previous }),
            Start::Pending => std::thread::sleep(tick),
            Start::Refused(why) => {
                child.stop();
                return Err(give_up(why));
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Windows process glue
// ---------------------------------------------------------------------------------------------------------------------

#[cfg(windows)]
pub mod win {
    use super::*;
    use std::io::Read;
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DONT_RESOLVE_DLL_REFERENCES: u32 = 0x0000_0001;
    const SEM_FAILCRITICALERRORS: u32 = 0x0001;
    const SEM_NOGPFAULTERRORBOX: u32 = 0x0002;
    const SEM_NOOPENFILEERRORBOX: u32 = 0x8000;

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryExW(name: *const u16, file: isize, flags: u32) -> isize;
        fn FreeLibrary(module: isize) -> i32;
        fn GetLastError() -> u32;
        fn SetErrorMode(mode: u32) -> u32;
        fn GetErrorMode() -> u32;
    }

    /// SetErrorMode is process-wide and inherited at CreateProcess: set → spawn → restore under one lock, adding our
    /// flags to whatever the process had (Astra review).
    static ERROR_MODE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn with_error_mode<T>(flags: u32, f: impl FnOnce() -> T) -> T {
        let _g = ERROR_MODE.lock().unwrap_or_else(|e| e.into_inner());
        let prev = unsafe { GetErrorMode() };
        unsafe { SetErrorMode(prev | flags) };
        let r = f();
        unsafe { SetErrorMode(prev) };
        r
    }

    fn wide(p: &Path) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
    }

    /// The first 4 KiB (enough for the PE header), or None when unreadable.
    fn head(p: &Path) -> Option<Vec<u8>> {
        let mut v = Vec::with_capacity(4096);
        std::fs::File::open(p).ok()?.take(4096).read_to_end(&mut v).ok()?;
        Some(v)
    }

    fn is_pe(p: &Path) -> bool {
        head(p).is_some_and(|h| h.starts_with(b"MZ"))
    }

    /// A program file by name: these must be real PE files (a truncated or emptied one is refused, not skipped).
    fn named_program(p: &Path) -> bool {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| ["dll", "exe", "pyd", "node"].iter().any(|x| e.eq_ignore_ascii_case(x)))
    }

    /// Every PE file under `dir` (plain files only; links are never followed) into `out`; anything the walk can't
    /// read, and program-named files that aren't PE, into `bad` (fail closed: Astra review).
    fn pe_files(dir: &Path, out: &mut Vec<PathBuf>, bad: &mut Vec<(PathBuf, i32)>) {
        let rd = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(e) => {
                bad.push((dir.to_path_buf(), e.raw_os_error().unwrap_or(-1)));
                return;
            }
        };
        for e in rd {
            let e = match e {
                Ok(e) => e,
                Err(err) => {
                    bad.push((dir.to_path_buf(), err.raw_os_error().unwrap_or(-1)));
                    continue;
                }
            };
            let p = e.path();
            let ft = match e.file_type() {
                Ok(ft) => ft,
                Err(err) => {
                    bad.push((p, err.raw_os_error().unwrap_or(-1)));
                    continue;
                }
            };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                pe_files(&p, out, bad);
            } else if ft.is_file() {
                if is_pe(&p) {
                    out.push(p);
                } else if named_program(&p) {
                    bad.push((p, 193)); // ERROR_BAD_EXE_FORMAT: a program file that isn't one
                }
            }
        }
    }

    fn same_path(a: &Path, b: &Path) -> bool {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    }

    /// The child side, `tinhat.exe --update-probe` (main.rs, first thing): map every PE file of this bundle once —
    /// as an image, so Windows applies the same code-integrity check as a real load, but without running any of its
    /// code (no DllMain, no imports) — print the report and exit (0 = all mapped). Writes nothing anywhere.
    pub fn run_probe() -> ! {
        unsafe { SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX) };
        let exe = std::env::current_exe().ok();
        let Some(dir) = exe.as_ref().and_then(|e| e.parent()).map(Path::to_path_buf) else {
            println!("{PROBE_TAG} done version= files=0 refused=0");
            std::process::exit(2);
        };
        let mut files = Vec::new();
        let mut bad = Vec::new();
        pe_files(&dir, &mut files, &mut bad);
        let mut n = 1; // tinhat.exe itself: Windows started it, or we wouldn't be here
        let mut refused: Vec<(String, i32)> =
            bad.iter().map(|(f, c)| (f.strip_prefix(&dir).unwrap_or(f).display().to_string(), *c)).collect();
        for f in files {
            if exe.as_deref().is_some_and(|e| same_path(e, &f)) {
                continue;
            }
            if head(&f).is_some_and(|h| !probe_maps(&h)) {
                let rel = f.strip_prefix(&dir).unwrap_or(&f).display().to_string();
                println!("{PROBE_TAG} skipped another-cpu file={rel}");
                continue;
            }
            n += 1;
            let w = wide(&f);
            let h = unsafe { LoadLibraryExW(w.as_ptr(), 0, DONT_RESOLVE_DLL_REFERENCES) };
            if h == 0 {
                let code = unsafe { GetLastError() } as i32;
                let rel = f.strip_prefix(&dir).unwrap_or(&f).display().to_string();
                refused.push((rel, code));
            } else {
                unsafe { FreeLibrary(h) };
            }
        }
        use std::io::Write;
        let mut o = std::io::stdout();
        let _ = o.write_all(probe_report(&bundle_version(&dir), n, &refused).as_bytes());
        let _ = o.flush();
        std::process::exit(if refused.is_empty() { 0 } else { 3 })
    }

    /// The parent side: run the staged bundle's probe and judge it.
    pub fn probe(l: &Layout, want: &str) -> Result<usize, Refusal> {
        #[cfg(feature = "lab-update-key")]
        if std::env::var("TINHAT_LAB_SKIP_PROBE").as_deref() == Ok("1") {
            eprintln!("[update] LAB BUILD: probe skipped (TINHAT_LAB_SKIP_PROBE=1)");
            return Ok(0);
        }
        let exe = l.staged().join(EXE);
        // no "bad image" / "unable to start" dialog from the probe (the mode is inherited at CreateProcess)
        let spawned = with_error_mode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX, || std::process::Command::new(&exe)
            .arg(PROBE_ARG)
            .current_dir(&l.root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .env_remove("TINHAT_RELAUNCH")
            .env_remove(crate::relaunch::ACK_ENV)
            .env_remove("TINHAT_UPDATE_TEST")
            .creation_flags(CREATE_NO_WINDOW)
            .spawn());
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => return judge_probe(Some(e.raw_os_error()), None, "", want),
        };
        let reader = child.stdout.take().map(|out| {
            std::thread::spawn(move || {
                let mut s = Vec::new();
                let _ = out.take(1 << 20).read_to_end(&mut s);
                String::from_utf8_lossy(&s).into_owned()
            })
        });
        let t0 = Instant::now();
        let exit = loop {
            match child.try_wait() {
                Ok(Some(st)) => break Some(st.code().unwrap_or(-1)),
                Ok(None) if t0.elapsed() < PROBE_TIMEOUT => std::thread::sleep(Duration::from_millis(100)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let out = reader.and_then(|h| h.join().ok()).unwrap_or_default();
        let r = judge_probe(None, exit, &out, want);
        match &r {
            Ok(n) => eprintln!("[update] the new version's check passed: {n} program files load on this PC"),
            Err(e) => {
                eprintln!("[update] the new version's check FAILED: {}", e.why());
                for line in out.lines().filter(|l| l.contains(" refused ")).take(20) {
                    eprintln!("[update]   {line}");
                }
            }
        }
        r
    }

    /// The started process.
    pub struct Child(pub std::process::Child);

    impl NewInstance for Child {
        fn exit_code(&mut self) -> Option<i32> {
            match self.0.try_wait() {
                Ok(Some(s)) => Some(s.code().unwrap_or(-1)),
                Ok(None) => None,
                Err(_) => Some(-1),
            }
        }
        fn stop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// A new instance that runs and waits for this one to quit.
    pub struct Handed {
        pub inner: HandedOver<Child>,
        ack: PathBuf,
        layout: Layout,
        /// given back or called off already (Drop has nothing left to do)
        done: bool,
    }

    /// A hand-over nobody committed (the Qt queue dropped it: the window is going away): the new instance stands down
    /// and a switched update goes back, so current\ never keeps an unsupervised candidate (Astra review). A commit
    /// ends the process with exit() and never runs this.
    impl Drop for Handed {
        fn drop(&mut self) {
            if self.done {
                return;
            }
            let _ = std::fs::remove_file(&self.ack);
            self.inner.child.stop();
            if self.inner.switched {
                let _ = roll_back(&self.layout, &Retry::default());
                let r = kept_in_place(&self.layout, &self.inner.previous, &Retry::default());
                eprintln!("[update] an unfinished hand-over was dropped: new instance stopped, folders back: {r:?}");
            }
        }
    }

    impl Handed {
        /// Right before this instance quits: the new one must still be there.
        pub fn still_running(&mut self) -> bool {
            self.inner.child.exit_code().is_none()
        }
        /// It answered but is gone before the hand-over: stop what's left and put the folders back (current\ never keeps
        /// a version whose start didn't hold).
        pub fn give_back(mut self) -> Result<(), String> {
            self.done = true;
            let _ = std::fs::remove_file(&self.ack);
            self.inner.child.stop();
            if self.inner.switched {
                let rb = roll_back(&self.layout, &Retry::default());
                kept_in_place(&self.layout, &self.inner.previous, &Retry::default()).map_err(|e| match rb {
                    Ok(()) => e,
                    Err(rb) => format!("{rb}; {e}"),
                })
            } else {
                Ok(())
            }
        }
        /// Called off (a call started meanwhile): the new instance stands down (relaunch::called_off), and a switched
        /// update goes back to staging, still ready for the next restart.
        pub fn call_off(mut self) -> Result<(), String> {
            self.done = true;
            let _ = std::fs::remove_file(&self.ack);
            let t0 = Instant::now();
            while self.inner.child.exit_code().is_none() && t0.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(Duration::from_millis(100));
            }
            if self.inner.child.exit_code().is_none() {
                self.inner.child.stop();
            }
            if self.inner.switched {
                // the final state decides (kept_in_place retries a move unswitch gave up on)
                if let Err(e) = unswitch(&self.layout, &Retry::default()) {
                    eprintln!("[update] call-off: {e}");
                }
                kept_in_place(&self.layout, &self.inner.previous, &Retry::default())
            } else {
                Ok(())
            }
        }
    }

    /// The supervised restart (update.rs restart() and the startup switch): switch in a ready update, start
    /// current\tinhat.exe with `args`, wait for its ack. `extra_env` = the headless gate's variables.
    pub fn hand_over(l: &Layout, running: &str, args: &[String], extra_env: &[(&str, &str)]) -> Result<Handed, NotStarted> {
        let (ack, token) = match crate::relaunch::new_ack_path_windows() {
            Ok(p) => p,
            Err(e) => {
                return Err(NotStarted {
                    refusal: Refusal::Failed(format!("no place for the start report: {e}")),
                    kept: running.to_string(),
                    rollback: Ok(()),
                })
            }
        };
        let r = switch_and_start(
            l,
            running,
            |exe| {
                // a DLL Windows refuses at load shows no dialog: the process just ends, and we see that
                let mut cmd = std::process::Command::new(exe);
                cmd.args(args)
                    .env("TINHAT_RELAUNCH", "update")
                    .env(crate::relaunch::ACK_ENV, &ack)
                    .envs(extra_env.iter().copied())
                    .current_dir(&l.root);
                // a startup in safe mode must not hand its software-rendering switches to the new version (it decides
                // its own mode; Astra review)
                if crate::boot::safe_mode() {
                    for v in ["TINHAT_SAFE_MODE_ACTIVE", "QT_QUICK_BACKEND", "LIBGL_ALWAYS_SOFTWARE"] {
                        cmd.env_remove(v);
                    }
                    if let Ok(f) = std::env::var("QTWEBENGINE_CHROMIUM_FLAGS") {
                        let rest: Vec<&str> = f.split_whitespace().filter(|w| *w != "--disable-gpu").collect();
                        cmd.env("QTWEBENGINE_CHROMIUM_FLAGS", rest.join(" "));
                    }
                }
                with_error_mode(SEM_FAILCRITICALERRORS, || cmd.spawn().map(Child))
            },
            || std::fs::read_to_string(&ack).map(|s| s.trim() == token).unwrap_or(false),
            START_TIMEOUT,
            Duration::from_millis(100),
            &Retry::default(),
        );
        match r {
            Ok(inner) => Ok(Handed { inner, ack, layout: l.clone(), done: false }),
            Err(e) => {
                let _ = std::fs::remove_file(&ack);
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn tmp(name: &str) -> Layout {
        let root = std::env::temp_dir().join(format!("tinhat-wus-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Layout::new(root)
    }
    fn bundle(dir: &Path, ver: &str) {
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::write(dir.join(EXE), format!("MZ exe {ver}")).unwrap();
        std::fs::write(dir.join("lib").join("a.dll"), format!("MZ dll {ver}")).unwrap();
        std::fs::write(dir.join("VERSION"), ver).unwrap();
    }
    /// installed 1.0 in current\ + a downloaded 2.0 staged and marked ready (the download step)
    fn installed_and_staged(name: &str) -> Layout {
        let l = tmp(name);
        bundle(&l.current(), "1.0");
        let unpack = prepare_staging(&l).unwrap();
        bundle(&unpack.join("tinhat-2.0-win64"), "2.0");
        adopt_unpacked(&l, "2.0").unwrap();
        mark_ready(&l, "2.0").unwrap();
        l
    }
    fn newer(a: &str, b: &str) -> bool {
        a > b
    }
    const FAST: Retry = Retry { tries: 1, pause: Duration::ZERO };

    #[derive(Debug)]
    struct Fake {
        exits_after: Option<(u32, i32)>,
        polls: u32,
        stopped: std::rc::Rc<Cell<bool>>,
    }
    impl NewInstance for Fake {
        fn exit_code(&mut self) -> Option<i32> {
            self.polls += 1;
            if self.stopped.get() {
                return Some(1);
            }
            match self.exits_after {
                Some((n, code)) if self.polls > n => Some(code),
                _ => None,
            }
        }
        fn stop(&mut self) {
            self.stopped.set(true);
        }
    }
    fn fake(exits_after: Option<(u32, i32)>) -> (Fake, std::rc::Rc<Cell<bool>>) {
        let s = std::rc::Rc::new(Cell::new(false));
        (Fake { exits_after, polls: 0, stopped: s.clone() }, s)
    }
    fn start(
        l: &Layout,
        spawn: impl FnOnce(&Path) -> std::io::Result<Fake>,
        acked: impl FnMut() -> bool,
    ) -> Result<HandedOver<Fake>, NotStarted> {
        switch_and_start(l, "1.0", spawn, acked, Duration::from_millis(50), Duration::from_millis(1), &FAST)
    }

    #[test]
    fn a_downloaded_update_never_touches_the_installed_app() {
        let l = installed_and_staged("download");
        assert_eq!(bundle_version(&l.current()), "1.0", "current\\ still holds the version that runs");
        assert!(!l.backup().exists(), "no switch at download time");
        assert_eq!(ready_version(&l).as_deref(), Some("2.0"));
    }

    #[test]
    fn a_new_version_windows_refuses_leaves_the_old_one_starting() {
        let l = installed_and_staged("refused");
        // CreateProcess fails: ERROR_SYSTEM_INTEGRITY_POLICY_VIOLATION (4551)
        let r = start(&l, |_| Err(std::io::Error::from_raw_os_error(4551)), || false).unwrap_err();
        assert!(matches!(r.refusal, Refusal::Blocked(_)), "{:?}", r.refusal);
        assert!(r.rollback.is_ok(), "{:?}", r.rollback);
        assert_eq!(bundle_version(&l.current()), "1.0", "every shortcut starts the old version again");
        assert!(l.current().join(EXE).is_file());
        assert!(!l.backup().exists() && !l.staging().exists(), "nothing left half-done");
        assert_eq!(r.refusal.message("1.0"), format!("Windows blocked this update, so Tin Hat stays on 1.0. You can keep using it. More: {HELP}"));
    }

    #[test]
    fn a_new_version_that_dies_in_the_loader_is_rolled_back() {
        let l = installed_and_staged("loader");
        let (f, _) = fake(Some((2, 0xC000_0428u32 as i32)));
        let r = start(&l, |_| Ok(f), || false).unwrap_err();
        assert!(matches!(r.refusal, Refusal::Blocked(_)), "{:?}", r.refusal);
        assert_eq!(bundle_version(&l.current()), "1.0");
    }

    #[test]
    fn a_new_version_that_never_reports_is_stopped_and_rolled_back() {
        let l = installed_and_staged("silent");
        let (f, stopped) = fake(None);
        let r = start(&l, |_| Ok(f), || false).unwrap_err();
        assert!(stopped.get(), "the silent process was stopped before the folders moved back");
        assert!(matches!(r.refusal, Refusal::Failed(_)));
        assert_eq!(bundle_version(&l.current()), "1.0");
    }

    #[test]
    fn started_then_gone_before_the_hand_over_is_not_started() {
        let l = installed_and_staged("acked-exit");
        let (f, _) = fake(Some((0, 1)));
        let r = start(&l, |_| Ok(f), || true).unwrap_err();
        assert!(matches!(r.refusal, Refusal::Failed(_)));
        assert_eq!(bundle_version(&l.current()), "1.0");
    }

    #[test]
    fn a_new_version_that_runs_takes_over() {
        let l = installed_and_staged("ok");
        let seen = Cell::new(0);
        let (f, stopped) = fake(None);
        let h = start(
            &l,
            |exe| {
                assert_eq!(exe, l.current().join(EXE));
                assert_eq!(bundle_version(&l.current()), "2.0", "started from current\\ after the switch");
                Ok(f)
            },
            || {
                seen.set(seen.get() + 1);
                seen.get() > 3
            },
        )
        .unwrap();
        assert!(h.switched && h.version == "2.0" && !stopped.get());
        assert_eq!(bundle_version(&l.current()), "2.0");
        assert_eq!(bundle_version(&l.backup()), "1.0", "backup\\ stays until the new version's next start sweeps it");
        assert_eq!(startup_plan(&l, "2.0", newer), StartupPlan::Sweep);
    }

    #[test]
    fn a_plain_restart_switches_nothing() {
        let l = tmp("plain");
        bundle(&l.current(), "1.0");
        let (f, _) = fake(None);
        let h = start(&l, |_| Ok(f), || true).unwrap();
        assert!(!h.switched && h.version == "1.0");
        let r = start(&l, |_| Err(std::io::Error::from_raw_os_error(2)), || false).unwrap_err();
        assert!(matches!(r.refusal, Refusal::Failed(_)) && r.rollback.is_ok());
        assert_eq!(bundle_version(&l.current()), "1.0");
    }

    #[test]
    fn a_called_off_hand_over_puts_the_update_back_in_staging() {
        let l = installed_and_staged("calloff");
        let (f, _) = fake(None);
        let h = start(&l, |_| Ok(f), || true).unwrap();
        assert!(h.switched);
        unswitch(&l, &FAST).unwrap();
        assert_eq!(bundle_version(&l.current()), "1.0");
        assert_eq!(ready_version(&l).as_deref(), Some("2.0"), "the next restart switches again");
    }

    #[test]
    fn a_locked_folder_moves_nothing() {
        let l = installed_and_staged("locked");
        std::fs::create_dir_all(l.backup().join("x")).unwrap();
        // a backup\ that can't be deleted = in use: simulate with a file where the folder must go
        std::fs::remove_dir_all(l.backup()).unwrap();
        std::fs::write(l.backup(), b"in the way").unwrap();
        let r = start(&l, |_| panic!("nothing may start"), || false).unwrap_err();
        assert!(matches!(r.refusal, Refusal::NotPlaced(_)), "{:?}", r.refusal);
        assert_eq!(bundle_version(&l.current()), "1.0");
        assert_eq!(ready_version(&l).as_deref(), Some("2.0"));
    }

    #[test]
    fn a_missing_installed_app_is_never_reported_as_kept() {
        let l = installed_and_staged("missing");
        // the shortcuts' folder vanished mid-attempt and nothing can put it back: say so
        std::fs::remove_dir_all(l.current()).unwrap();
        assert!(kept_in_place(&l, "1.0", &FAST).is_err());
        // backup\ still has the old version: it goes back
        let l = installed_and_staged("missing2");
        switch(&l, &FAST).unwrap();
        std::fs::remove_dir_all(l.current()).unwrap();
        assert!(kept_in_place(&l, "1.0", &FAST).is_ok());
        assert_eq!(bundle_version(&l.current()), "1.0");
        // the candidate left in current\ is not "kept"
        let l = installed_and_staged("missing3");
        switch(&l, &FAST).unwrap();
        assert!(kept_in_place(&l, "1.0", &FAST).unwrap_err().contains("2.0"));
    }

    #[test]
    fn startup_switches_only_to_a_complete_probed_newer_bundle() {
        let l = installed_and_staged("plan");
        assert_eq!(startup_plan(&l, "1.0", newer), StartupPlan::Switch("2.0".into()));
        assert_eq!(startup_plan(&l, "2.0", newer), StartupPlan::Sweep, "not newer");
        std::fs::write(l.staged().join("VERSION"), "2.1").unwrap();
        assert_eq!(startup_plan(&l, "1.0", newer), StartupPlan::Sweep, "READY and the bundle disagree");
        let l = tmp("plan-unprobed");
        bundle(&l.current(), "1.0");
        bundle(&prepare_staging(&l).unwrap(), "2.0");
        adopt_unpacked(&l, "2.0").unwrap();
        assert_eq!(startup_plan(&l, "1.0", newer), StartupPlan::Sweep, "never probed = never switched");
        sweep(&l);
        assert!(!l.staging().exists());
    }

    #[test]
    fn a_zip_with_the_wrong_version_is_not_staged() {
        let l = tmp("wrongver");
        bundle(&l.current(), "1.0");
        bundle(&prepare_staging(&l).unwrap(), "1.9");
        assert!(adopt_unpacked(&l, "2.0").unwrap_err().contains("1.9"));
        assert!(ready_version(&l).is_none());
    }

    fn pe(machine: u16) -> Vec<u8> {
        let mut v = vec![0u8; 0x200];
        v[..2].copy_from_slice(b"MZ");
        v[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        v[0x80..0x84].copy_from_slice(b"PE\0\0");
        v[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
        v
    }

    #[test]
    fn the_probe_skips_only_well_formed_programs_for_another_cpu() {
        assert_eq!(pe_machine(&pe(0x8664)), Some(0x8664));
        assert!(probe_maps(&pe(NATIVE_MACHINE)));
        assert!(!probe_maps(&pe(0x014c)), "a 32-bit exe (vc_redist's bootstrapper) is not mapped");
        // the planted file ("MZ" + filler) and anything malformed is still handed to Windows (and refused there)
        let planted: Vec<u8> = [b"MZ".as_slice(), &b"\0planted\0".repeat(64)].concat();
        assert_eq!(pe_machine(&planted), None);
        assert!(probe_maps(&planted) && probe_maps(b"MZ") && probe_maps(b""));
        let mut far = pe(0x014c);
        far[0x3c..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(probe_maps(&far), "an offset past the end is not a well-formed header");
    }

    #[test]
    fn the_probe_report_round_trips() {
        assert_eq!(judge_probe(None, Some(0), &probe_report("2.0", 412, &[]), "2.0"), Ok(412));
        let r = probe_report("2.0", 412, &[("lib\\gstreamer-1.0\\gstfoo.dll".into(), 4551), ("b.dll".into(), 4551)]);
        let e = judge_probe(None, Some(3), &r, "2.0").unwrap_err();
        assert!(matches!(&e, Refusal::Blocked(w) if w.contains("gstfoo.dll") && w.contains("1 more")), "{e:?}");
        let r = probe_report("2.0", 3, &[("broken.dll".into(), 193)]);
        assert!(matches!(judge_probe(None, Some(3), &r, "2.0"), Err(Refusal::Failed(w)) if w.contains("broken.dll")));
    }

    #[test]
    fn a_probe_counts_only_with_a_clean_exit_and_the_right_report() {
        let ok = probe_report("2.0", 9, &[]);
        assert!(matches!(judge_probe(Some(Some(4551)), None, "", "2.0"), Err(Refusal::Blocked(_))));
        assert!(matches!(judge_probe(Some(Some(193)), None, "", "2.0"), Err(Refusal::Failed(_))));
        assert!(matches!(judge_probe(None, None, &ok, "2.0"), Err(Refusal::Failed(w)) if w.contains("didn't finish")));
        assert!(matches!(judge_probe(None, Some(0xC000_0428u32 as i32), "", "2.0"), Err(Refusal::Blocked(_))));
        assert!(matches!(judge_probe(None, Some(1), &ok, "2.0"), Err(Refusal::Failed(_))), "exit 1 even with a report");
        assert!(judge_probe(None, Some(0), "", "2.0").is_err(), "silent = not ok (e.g. an old build that ignores the flag)");
        assert!(judge_probe(None, Some(0), &ok, "2.1").is_err(), "another version's report");
        assert!(judge_probe(None, Some(0), &probe_report("2.0", 0, &[]), "2.0").is_err());
    }

    #[test]
    fn judge_start_rules() {
        let t = Duration::from_secs(30);
        assert_eq!(judge_start(None, true, Duration::ZERO, t), Start::Started);
        assert_eq!(judge_start(None, false, Duration::from_secs(1), t), Start::Pending);
        assert!(matches!(judge_start(None, false, t, t), Start::Refused(Refusal::Failed(_))));
        assert!(matches!(judge_start(Some(0), true, Duration::ZERO, t), Start::Refused(_)), "acked but gone");
        assert!(matches!(judge_start(Some(0xC000_0428u32 as i32), false, Duration::ZERO, t), Start::Refused(Refusal::Blocked(_))));
    }

    #[test]
    fn the_messages_are_plain() {
        let f = Refusal::Failed("x".into()).message("12.1.13");
        assert_eq!(f, format!("This update didn't start on this PC, so Tin Hat stays on 12.1.13. You can keep using it. More: {HELP}"));
        assert!(Refusal::NotPlaced("x".into()).message("").contains("stays on the version you have"));
        assert!(is_policy_code(4551) && is_policy_code(577) && !is_policy_code(2) && !is_policy_code(193));
    }
}
