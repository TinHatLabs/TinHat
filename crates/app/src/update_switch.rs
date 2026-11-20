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
    if switch_unsettled(l) {
        // 12.1.17: staging\ holds the record of a switch that isn't settled (second review: never delete it here)
        return Err("an earlier update isn't finished yet; restart Tin Hat and try again".to_string());
    }
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
/// bundle's VERSION says the same. 12.1.17: when the download kept the installed copies of its unchanged files
/// ([`plan_unchanged`]), that plan must still fit current\ (same version, every listed file there at its size).
pub fn ready_version(l: &Layout) -> Option<String> {
    let v = std::fs::read_to_string(l.ready_file()).ok()?.trim().to_string();
    if v.is_empty() || !l.staged().join(EXE).is_file() || bundle_version(&l.staged()) != v {
        return None;
    }
    match read_plan(l) {
        Ok(None) => Some(v),
        Ok(Some(p)) if p.fits(l, &v) => Some(v),
        Ok(Some(_)) => {
            eprintln!("[update] the staged update no longer matches the installed app (it changed since the download)");
            None
        }
        Err(e) => {
            eprintln!("[update] the staged update's file list is unreadable ({e})");
            None
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// 12.1.17: unchanged files are never written again (Smart App Control)
// ---------------------------------------------------------------------------------------------------------------------
//
// Smart App Control judges every NEW file on its own. An update rewrote all ~2,400 files of the bundle although only
// tinhat.exe and VERSION change in a normal release, and on a SAC-On PC fresh copies of byte-identical DLLs were
// refused while the installed ones loaded (win-fresh-sac r2: 12.1.16 never opened after an in-app update, 5/5). So
// the download now compares the new bundle with current\ and drops its copy of every byte-identical file; the plan
// file (staging\UNCHANGED) lists them, and the switch keeps the installed files instead of fresh copies.

/// The plan file in staging\ (next to READY, outside the bundle folder).
const PLAN_FILE: &str = "UNCHANGED";
const PLAN_MAGIC: &str = "tinhat-unchanged 1";

/// One installed file the new version keeps as it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kept {
    /// relative to the bundle folder, '/'-separated
    pub rel: String,
    pub len: u64,
    pub sha256: String,
}

/// staging\UNCHANGED: the version the download was compared against, the version it brings, and the kept files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub from: String,
    pub to: String,
    pub files: Vec<Kept>,
}

impl Plan {
    /// Still true for current\: it holds `from`, the plan brings `to`, and every kept file is there at its size.
    pub fn fits(&self, l: &Layout, to: &str) -> bool {
        self.to == to
            && !self.from.is_empty()
            && bundle_version(&l.current()) == self.from
            && self.files.iter().all(|k| {
                std::fs::symlink_metadata(rel_path(&l.current(), &k.rel)).is_ok_and(|m| m.is_file() && m.len() == k.len)
            })
    }
}

impl Layout {
    fn plan_file(&self) -> PathBuf {
        self.staging().join(PLAN_FILE)
    }
}

/// A plan entry's path under `base`. Only plain relative names are accepted (no "..", ".", drive, stream or root).
fn safe_rel(rel: &str) -> bool {
    !rel.is_empty()
        && !rel.contains(['\\', ':', '\0'])
        && !rel.starts_with('/')
        && rel.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

fn rel_path(base: &Path, rel: &str) -> PathBuf {
    let mut p = base.to_path_buf();
    p.extend(rel.split('/'));
    p
}

/// What a bundle folder holds: relative, '/'-separated, exact names, sorted. Links (and anything that is neither a
/// file nor a folder) are listed apart and never followed.
#[derive(Debug, Default)]
struct Survey {
    files: Vec<String>,
    dirs: Vec<String>,
    links: Vec<String>,
}

fn survey(dir: &Path) -> Result<Survey, String> {
    fn rel_of(base: &Path, p: &Path) -> Result<String, String> {
        let rel = p.strip_prefix(base).map_err(|e| e.to_string())?;
        Ok(rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/"))
    }
    fn go(base: &Path, dir: &Path, s: &mut Survey) -> Result<(), String> {
        for e in std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
            let e = e.map_err(|e| format!("read {}: {e}", dir.display()))?;
            let p = e.path();
            let ft = e.file_type().map_err(|err| format!("{}: {err}", p.display()))?;
            let rel = rel_of(base, &p)?;
            if ft.is_symlink() {
                s.links.push(rel);
            } else if ft.is_dir() {
                s.dirs.push(rel);
                go(base, &p, s)?;
            } else if ft.is_file() {
                s.files.push(rel);
            } else {
                s.links.push(rel);
            }
        }
        Ok(())
    }
    if !plain_dir(dir) {
        return Err(format!("{} is not a plain folder", dir.display()));
    }
    let mut s = Survey::default();
    go(dir, dir, &mut s)?;
    s.files.sort();
    s.dirs.sort();
    Ok(s)
}

/// A real folder, not a link or junction.
fn plain_dir(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
}

/// Key for a name as Windows compares them: each character through its 1:1 uppercase mapping (NTFS's upcase table;
/// characters whose uppercase is longer stay as they are). Second review: Unicode lowercasing differs (ς/σ/Σ).
fn key(rel: &str) -> String {
    rel.chars()
        .map(|c| {
            let mut u = c.to_uppercase();
            match (u.next(), u.next()) {
                (Some(x), None) => x,
                _ => c,
            }
        })
        .collect()
}

/// Why the installed folder and the downloaded one can't be switched in place (None = they can). In place only for
/// plain bundles: no links or junctions, no file that becomes a folder or back, no two names that differ only in case.
/// Anything else keeps every downloaded file and switches the whole folder, as before 12.1.17 (second review: these
/// cases made the undo ambiguous).
fn in_place_blocker(cur: &Survey, new: &Survey) -> Option<String> {
    if let Some(x) = cur.links.first() {
        return Some(format!("a link in the installed app ({x})"));
    }
    if let Some(x) = new.links.first() {
        return Some(format!("a link in the update ({x})"));
    }
    for s in [cur, new] {
        let mut seen = std::collections::HashSet::new();
        if let Some(x) = s.files.iter().chain(&s.dirs).find(|x| !seen.insert(key(x))) {
            return Some(format!("two names that differ only in case ({x})"));
        }
    }
    let index = |s: &Survey, files: bool| -> std::collections::HashMap<String, String> {
        (if files { &s.files } else { &s.dirs }).iter().map(|x| (key(x), x.clone())).collect()
    };
    let (cf, cd) = (index(cur, true), index(cur, false));
    for (names, is_file) in [(&new.files, true), (&new.dirs, false)] {
        for x in names {
            let k = key(x);
            let (same, other) = if is_file { (&cf, &cd) } else { (&cd, &cf) };
            if other.contains_key(&k) {
                return Some(format!("{x} changes between a file and a folder"));
            }
            if same.get(&k).is_some_and(|e| e != x) {
                return Some(format!("{x} changes only in case"));
            }
        }
    }
    None
}

fn file_sha256(p: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The probe child's PATH when the staged bundle keeps installed files: current\ first, then the inherited PATH.
pub fn probe_path(l: &Layout) -> Option<std::ffi::OsString> {
    let plan = read_plan(l).ok().flatten()?;
    if plan.files.is_empty() {
        return None;
    }
    let mut paths = vec![l.current()];
    if let Some(old) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&old));
    }
    std::env::join_paths(paths).ok()
}

/// Never kept: the program itself and the version stamp (a release always brings its own).
fn always_fresh(rel: &str) -> bool {
    rel.eq_ignore_ascii_case(EXE) || rel.eq_ignore_ascii_case("VERSION")
}

/// The plan, if the staged update has one. Err = a plan file that isn't one of ours (the update is then not used).
pub fn read_plan(l: &Layout) -> Result<Option<Plan>, String> {
    let s = match std::fs::read_to_string(l.plan_file()) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut lines = s.lines();
    if lines.next() != Some(PLAN_MAGIC) {
        return Err("not a plan file".into());
    }
    let from = lines.next().and_then(|x| x.strip_prefix("from ")).ok_or("no from line")?.to_string();
    let to = lines.next().and_then(|x| x.strip_prefix("to ")).ok_or("no to line")?.to_string();
    let mut files = Vec::new();
    for line in lines {
        let mut it = line.splitn(3, ' ');
        let (Some(len), Some(sha), Some(rel)) = (it.next(), it.next(), it.next()) else {
            return Err(format!("bad line {line:?}"));
        };
        let len: u64 = len.parse().map_err(|_| format!("bad size in {line:?}"))?;
        if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) || !safe_rel(rel) || always_fresh(rel) {
            return Err(format!("bad entry {line:?}"));
        }
        files.push(Kept { rel: rel.to_string(), len, sha256: sha.to_string() });
    }
    Ok(Some(Plan { from, to, files }))
}

fn write_plan(l: &Layout, p: &Plan) -> Result<(), String> {
    let mut s = format!("{PLAN_MAGIC}\nfrom {}\nto {}\n", p.from, p.to);
    for k in &p.files {
        s.push_str(&format!("{} {} {}\n", k.len, k.sha256, k.rel));
    }
    write_durable(&l.staging().join("UNCHANGED.tmp"), &l.plan_file(), s.as_bytes())
        .map_err(|e| format!("write the file list: {e}"))
}

/// Download step, part 2b (12.1.17; after [`adopt_unpacked`], before the probe): every file of the staged bundle that
/// is byte-identical to the installed one (the same exact name, size and sha256; never tinhat.exe or VERSION) is listed
/// in staging\UNCHANGED and its fresh copy is deleted from staging\bundle, so it is never written into the new version.
/// The list is written (and synced) before anything is deleted. current\ is only read. Ok = how many files are kept
/// (0 = none, or the two folders aren't plain enough for an in-place switch: then every file stays, the old way).
pub fn plan_unchanged(l: &Layout) -> Result<usize, String> {
    let from = bundle_version(&l.current());
    let to = bundle_version(&l.staged());
    if from.is_empty() || to.is_empty() || !l.current().join(EXE).is_file() {
        return Ok(0); // nothing to compare against: the update brings every file (the old way)
    }
    let cur = match survey(&l.current()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[update] the installed app can't be read through ({e}): the whole folder is switched");
            return Ok(0);
        }
    };
    let new = survey(&l.staged())?;
    if let Some(why) = in_place_blocker(&cur, &new) {
        eprintln!("[update] {why}: every file of the update is kept and the whole folder is switched");
        return Ok(0);
    }
    let installed: std::collections::HashSet<&str> = cur.files.iter().map(String::as_str).collect();
    let mut files = Vec::new();
    for rel in &new.files {
        if always_fresh(rel) || !safe_rel(rel) || !installed.contains(rel.as_str()) {
            continue;
        }
        let (newp, oldp) = (rel_path(&l.staged(), rel), rel_path(&l.current(), rel));
        let (Ok(nm), Ok(om)) = (std::fs::symlink_metadata(&newp), std::fs::symlink_metadata(&oldp)) else { continue };
        if !om.is_file() || nm.len() != om.len() {
            continue;
        }
        let a = file_sha256(&newp).map_err(|e| format!("read {}: {e}", newp.display()))?;
        let Ok(b) = file_sha256(&oldp) else { continue }; // unreadable installed file: take the new copy
        if a == b {
            files.push(Kept { rel: rel.clone(), len: nm.len(), sha256: a });
        }
    }
    if files.is_empty() {
        return Ok(0);
    }
    let plan = Plan { from, to, files };
    write_plan(l, &plan)?;
    for k in &plan.files {
        std::fs::remove_file(rel_path(&l.staged(), &k.rel)).map_err(|e| format!("drop the copy of {}: {e}", k.rel))?;
    }
    Ok(plan.files.len())
}

// The in-place switch (12.1.17). current\ is NOT renamed: only the files the update changes move. Each old file that
// leaves goes to backup\ (same path), each new one comes from staging\bundle; files the new version no longer has
// go to backup\ too; folders only the new version has are created. tinhat.exe moves last, so the shortcuts never start
// the new program next to old files it would trip on. Before the first move, staging\SWITCHED records it all: each new
// file's size + sha256 and whether an old file of that name existed, each removed file, each created folder. Anything
// that stops the switch (an error here, a refused start, a call-off, the old process dying) is undone from that
// record: a file is taken out of current\ only when it IS the new file, so an undo can run again safely, and the files
// the update kept are never touched. An undo first writes staging\UNDOING: an undo that stopped half way is finished
// at the next start whichever program that start is. The record is removed (settled) only by the NEW version once its
// window is up (never while UNDOING exists), or by a complete undo (record first, then backup\).

const JOURNAL_FILE: &str = "SWITCHED";
const JOURNAL_MAGIC: &str = "tinhat-switch 4";
const UNDOING_FILE: &str = "UNDOING";

impl Layout {
    fn journal(&self) -> PathBuf {
        self.staging().join(JOURNAL_FILE)
    }
    fn undoing(&self) -> PathBuf {
        self.staging().join(UNDOING_FILE)
    }
}

/// One new file of an in-place switch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Incoming {
    pub file: Kept,
    /// the installed version's file of this name (size + sha256), if it had one (it goes to backup\ and back)
    pub old: Option<Kept>,
}

/// What an in-place switch moves (staging\SWITCHED).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Journal {
    pub from: String,
    pub to: String,
    /// the new version's files from staging\bundle, in move order (tinhat.exe last)
    pub incoming: Vec<Incoming>,
    /// the old version's files the new one doesn't have (they go to backup\)
    pub outgoing: Vec<String>,
    /// folders the new version has and the old one hadn't (created in current\; parents first)
    pub created: Vec<String>,
}

/// An in-place switch has started and isn't settled yet.
pub fn switch_unsettled(l: &Layout) -> bool {
    l.journal().exists()
}

/// One switch, undo or sweep at a time in this process (second review: the old instance's own late backup\ sweep ran
/// on another thread while its switch moved files).
static TXN: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn txn() -> std::sync::MutexGuard<'static, ()> {
    TXN.lock().unwrap_or_else(|e| e.into_inner())
}

/// The install roots this process started a switch in (the old instance, until it quits or puts the files back): its
/// own sweeps must never settle that switch. Per root, so parallel tests don't see each other's.
static SWITCHING_HERE: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

fn mark_switching_here(l: &Layout, on: bool) {
    let mut v = SWITCHING_HERE.lock().unwrap_or_else(|e| e.into_inner());
    v.retain(|r| r != &l.root);
    if on {
        v.push(l.root.clone());
    }
}

fn switching_here(l: &Layout) -> bool {
    SWITCHING_HERE.lock().unwrap_or_else(|e| e.into_inner()).contains(&l.root)
}

pub fn read_journal(l: &Layout) -> Result<Option<Journal>, String> {
    let s = match std::fs::read_to_string(l.journal()) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut lines = s.lines();
    if lines.next() != Some(JOURNAL_MAGIC) {
        return Err("not a switch record".into());
    }
    let from = lines.next().and_then(|x| x.strip_prefix("from ")).ok_or("no from line")?.to_string();
    let to = lines.next().and_then(|x| x.strip_prefix("to ")).ok_or("no to line")?.to_string();
    let (mut incoming, mut outgoing, mut created) = (Vec::new(), Vec::new(), Vec::new());
    for line in lines {
        if let Some(r) = line.strip_prefix("in ") {
            let mut it = r.splitn(4, ' ');
            let (Some(len), Some(sha), Some(old), Some(rel)) = (it.next(), it.next(), it.next(), it.next()) else {
                return Err(format!("bad line {line:?}"));
            };
            let hex = |x: &str| x.len() == 64 && x.bytes().all(|b| b.is_ascii_hexdigit());
            let len: u64 = len.parse().map_err(|_| format!("bad size in {line:?}"))?;
            if !hex(sha) || !safe_rel(rel) {
                return Err(format!("bad entry {line:?}"));
            }
            let old = match old {
                "-" => None,
                o => {
                    let (ol, os) = o.split_once(':').ok_or_else(|| format!("bad entry {line:?}"))?;
                    let ol: u64 = ol.parse().map_err(|_| format!("bad size in {line:?}"))?;
                    if !hex(os) {
                        return Err(format!("bad entry {line:?}"));
                    }
                    Some(Kept { rel: rel.to_string(), len: ol, sha256: os.to_string() })
                }
            };
            incoming.push(Incoming { file: Kept { rel: rel.to_string(), len, sha256: sha.to_string() }, old });
        } else if let Some(rel) = line.strip_prefix("out ") {
            if !safe_rel(rel) {
                return Err(format!("bad path {rel:?}"));
            }
            outgoing.push(rel.to_string());
        } else if let Some(rel) = line.strip_prefix("dir ") {
            if !safe_rel(rel) {
                return Err(format!("bad path {rel:?}"));
            }
            created.push(rel.to_string());
        } else {
            return Err(format!("bad line {line:?}"));
        }
    }
    Ok(Some(Journal { from, to, incoming, outgoing, created }))
}

/// Write a small file and make it durable (the handle that wrote it is the one synced: FlushFileBuffers needs write
/// access on Windows; second review), then rename it into place.
fn write_durable(tmp: &Path, dst: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(tmp, dst)
}

fn write_journal(l: &Layout, j: &Journal) -> Result<(), String> {
    let mut s = format!("{JOURNAL_MAGIC}\nfrom {}\nto {}\n", j.from, j.to);
    for i in &j.incoming {
        let old = i.old.as_ref().map(|o| format!("{}:{}", o.len, o.sha256)).unwrap_or_else(|| "-".into());
        s.push_str(&format!("in {} {} {} {}\n", i.file.len, i.file.sha256, old, i.file.rel));
    }
    for rel in &j.outgoing {
        s.push_str(&format!("out {rel}\n"));
    }
    for rel in &j.created {
        s.push_str(&format!("dir {rel}\n"));
    }
    write_durable(&l.staging().join("SWITCHED.tmp"), &l.journal(), s.as_bytes())
        .map_err(|e| format!("write the switch record: {e}"))
}

/// The new version runs (its window is up, or the verify phase): the switch can no longer be undone. Removes only the
/// record; false = it couldn't, or an undo is under way (then nothing else may be swept).
fn settle_switch(l: &Layout) -> bool {
    if !l.journal().exists() {
        return true;
    }
    if l.undoing().exists() {
        eprintln!("[update] an undo of the switch is under way: nothing is settled");
        return false;
    }
    match std::fs::remove_file(l.journal()) {
        Ok(()) => {
            eprintln!("[update] the switch to this version is settled");
            true
        }
        Err(e) => {
            eprintln!("[update] couldn't settle the switch record yet ({e}); backup\\ and staging\\ stay");
            false
        }
    }
}

/// No folder from `base` down to `p`'s parent is a link or junction (`base` included): a record path never leads out.
fn inside(base: &Path, p: &Path) -> bool {
    let Ok(rest) = p.strip_prefix(base) else { return false };
    if !plain_dir(base) {
        return false;
    }
    let mut cur = base.to_path_buf();
    let comps: Vec<_> = rest.components().collect();
    for c in comps.iter().take(comps.len().saturating_sub(1)) {
        cur.push(c);
        match std::fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() || !m.is_dir() => return false,
            _ => {} // missing folders are created by move_file (inside base)
        }
    }
    true
}

fn move_file(l: &Layout, from: &Path, to: &Path, r: &Retry) -> Result<(), String> {
    // backup\ (or a staging\bundle emptied by the switch) may not exist yet: the destination's base folder first
    if let Some(b) = [l.backup(), l.staged()].into_iter().find(|b| to.starts_with(b)) {
        if !exists(&b) {
            std::fs::create_dir_all(&b).map_err(|e| format!("mkdir {}: {e}", b.display()))?;
        }
    }
    for p in [from, to] {
        let base = [l.current(), l.backup(), l.staged()].into_iter().find(|b| p.starts_with(b));
        if !base.is_some_and(|b| inside(&b, p)) {
            return Err(format!("{} is not a plain path inside the app's folders", p.display()));
        }
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    rename_retry(from, to, r)
}

fn is_exe(rel: &str) -> bool {
    rel.eq_ignore_ascii_case(EXE)
}

/// Is the regular file at `p` exactly `k`? Ok(false) = another file; Err = it can't be read (never guessed: second
/// review).
fn is_file_of(p: &Path, k: &Kept) -> Result<bool, String> {
    let m = std::fs::symlink_metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
    if !m.is_file() {
        return Ok(false);
    }
    if m.len() != k.len {
        return Ok(false);
    }
    file_sha256(p).map(|h| h.eq_ignore_ascii_case(&k.sha256)).map_err(|e| format!("read {}: {e}", p.display()))
}

fn exists(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// Ok(false) only when the path really isn't there (NotFound); any other error is an error (third review).
fn present(p: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(p) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}

/// The record of an in-place switch for `plan`: the two folders re-checked (still plain, the kept files still the very
/// bytes the download compared: a repair or another build of the same version could have changed one since), the new
/// files hashed, the removed files and created folders listed.
fn make_journal(l: &Layout, plan: &Plan) -> Result<Journal, String> {
    for k in &plan.files {
        if !is_file_of(&rel_path(&l.current(), &k.rel), k).unwrap_or(false) {
            return Err(format!("the installed app changed since the update was downloaded ({})", k.rel));
        }
    }
    let (cur, new) = (survey(&l.current())?, survey(&l.staged())?);
    if let Some(why) = in_place_blocker(&cur, &new) {
        return Err(format!("the installed app changed since the update was downloaded ({why})"));
    }
    if !new.files.iter().any(|x| is_exe(x)) {
        return Err("nothing staged".to_string());
    }
    let had: std::collections::HashSet<&str> = cur.files.iter().map(String::as_str).collect();
    let mut names = new.files.clone();
    names.sort_by_key(|x| (is_exe(x), x.eq_ignore_ascii_case("VERSION"), x.clone())); // tinhat.exe last
    let mut incoming = Vec::new();
    for rel in names {
        let p = rel_path(&l.staged(), &rel);
        let len = std::fs::symlink_metadata(&p).map_err(|e| format!("{rel}: {e}"))?.len();
        let sha256 = file_sha256(&p).map_err(|e| format!("read {rel}: {e}"))?;
        let old = if had.contains(rel.as_str()) {
            let o = rel_path(&l.current(), &rel);
            let ol = std::fs::symlink_metadata(&o).map_err(|e| format!("{rel}: {e}"))?.len();
            let os = file_sha256(&o).map_err(|e| format!("read the installed {rel}: {e}"))?;
            Some(Kept { rel: rel.clone(), len: ol, sha256: os })
        } else {
            None
        };
        incoming.push(Incoming { file: Kept { rel, len, sha256 }, old });
    }
    let known: std::collections::HashSet<&str> =
        plan.files.iter().map(|k| k.rel.as_str()).chain(incoming.iter().map(|i| i.file.rel.as_str())).collect();
    let outgoing: Vec<String> = cur.files.iter().filter(|x| !known.contains(x.as_str())).cloned().collect();
    let old_dirs: std::collections::HashSet<&str> = cur.dirs.iter().map(String::as_str).collect();
    let created: Vec<String> = new.dirs.iter().filter(|d| !old_dirs.contains(d.as_str())).cloned().collect();
    Ok(Journal { from: plan.from.clone(), to: plan.to.clone(), incoming, outgoing, created })
}

/// The in-place switch; `fail_at` = stop with an error before move number N (tests: a switch cut off half way).
fn switch_in_place(l: &Layout, plan: &Plan, r: &Retry, fail_at: Option<usize>) -> Result<(), String> {
    let _t = txn();
    let j = make_journal(l, plan)?;
    mark_switching_here(l, true);
    if let Err(e) = write_journal(l, &j) {
        mark_switching_here(l, false);
        let _ = std::fs::remove_file(l.journal());
        return Err(e);
    }
    match switch_moves(l, &j, r, fail_at) {
        Ok(()) => {
            eprintln!(
                "[update] switched in place: {} files new or changed, {} kept untouched, {} removed",
                j.incoming.len(),
                plan.files.len(),
                j.outgoing.len()
            );
            Ok(())
        }
        Err(e) => Err(match undo_locked(l, r) {
            Ok(()) => format!("{e} (the installed app is back)"),
            Err(u) => format!("{e}; PUTTING THE INSTALLED FILES BACK FAILED: {u}"),
        }),
    }
}

/// The moves of an in-place switch, in order (the record is already written). Stops at the first error, undoing
/// nothing (the caller undoes; a cut-off process leaves this state for [`recover_interrupted`]).
fn switch_moves(l: &Layout, j: &Journal, r: &Retry, fail_at: Option<usize>) -> Result<(), String> {
    let mut moves: Vec<(PathBuf, PathBuf, String)> = Vec::new();
    for rel in &j.outgoing {
        moves.push((rel_path(&l.current(), rel), rel_path(&l.backup(), rel), format!("move out {rel}")));
    }
    // the folders the new version has (empty ones included: the whole-folder switch brought them too)
    for d in &j.created {
        let p = rel_path(&l.current(), d);
        if !inside(&l.current(), &p) {
            return Err(format!("{d} is not a plain path inside the app's folder"));
        }
        std::fs::create_dir_all(&p).map_err(|e| format!("mkdir {d}: {e}"))?;
    }
    for i in &j.incoming {
        let rel = &i.file.rel;
        let (cur, bak, new) = (rel_path(&l.current(), rel), rel_path(&l.backup(), rel), rel_path(&l.staged(), rel));
        if i.old.is_some() {
            moves.push((cur.clone(), bak, format!("move aside {rel}")));
        }
        moves.push((new, cur, format!("put {rel} in place")));
    }
    for (n, (from, to, what)) in moves.iter().enumerate() {
        if fail_at == Some(n) {
            return Err(format!("stopped before move {n} (test)"));
        }
        move_file(l, from, to, r).map_err(|e| format!("{what}: {e}"))?;
    }
    Ok(())
}

/// Undo an in-place switch from its record (see the section comment). Ok only when every file the record names is
/// where the old version had it; then the record goes FIRST, then UNDOING and backup\. staging\ (READY + the plan)
/// stays, so a call-off can switch again later.
pub fn undo_in_place(l: &Layout, r: &Retry) -> Result<(), String> {
    let _t = txn();
    undo_locked(l, r)
}

fn undo_locked(l: &Layout, r: &Retry) -> Result<(), String> {
    let j = read_journal(l)?.ok_or("no switch record")?;
    write_durable(&l.staging().join("UNDOING.tmp"), &l.undoing(), b"undoing\n")
        .map_err(|e| format!("mark the undo: {e}"))?;
    let mut errors: Vec<String> = Vec::new();
    // presence that can't be established is an error, never "absent" (third review)
    let here = |p: &Path, errors: &mut Vec<String>| -> Option<bool> {
        match present(p) {
            Ok(b) => Some(b),
            Err(e) => {
                errors.push(e);
                None
            }
        }
    };
    // the new file goes back to staging\bundle only when it IS the new file
    let take_out = |i: &Incoming, errors: &mut Vec<String>| {
        let (cur, new) = (rel_path(&l.current(), &i.file.rel), rel_path(&l.staged(), &i.file.rel));
        let (Some(n), Some(c)) = (here(&new, errors), here(&cur, errors)) else { return };
        if n || !c {
            return;
        }
        match is_file_of(&cur, &i.file) {
            Ok(true) => {
                if let Err(e) = move_file(l, &cur, &new, r) {
                    errors.push(format!("take {} out: {e}", i.file.rel));
                }
            }
            Ok(false) => {} // the old file (already back)
            Err(e) => errors.push(format!("can't tell which {} is in place: {e}", i.file.rel)),
        }
    };
    let put_back = |rel: &str, errors: &mut Vec<String>| {
        let (cur, bak) = (rel_path(&l.current(), rel), rel_path(&l.backup(), rel));
        let (Some(b), Some(c)) = (here(&bak, errors), here(&cur, errors)) else { return };
        if b && !c {
            if let Err(e) = move_file(l, &bak, &cur, r) {
                errors.push(format!("put the previous {rel} back: {e}"));
            }
        } else if b && c {
            // something new sits where the previous file must go back: never overwritten, never guessed
            errors.push(format!("{rel} is occupied while the previous one is still in backup\\"));
        }
    };
    // the program first, as one step
    for i in j.incoming.iter().filter(|i| is_exe(&i.file.rel)) {
        take_out(i, &mut errors);
        put_back(&i.file.rel, &mut errors);
    }
    let rest: Vec<&Incoming> = j.incoming.iter().rev().filter(|i| !is_exe(&i.file.rel)).collect();
    for i in &rest {
        take_out(i, &mut errors);
    }
    for i in &rest {
        put_back(&i.file.rel, &mut errors);
    }
    for rel in &j.outgoing {
        put_back(rel, &mut errors);
    }
    // the folders the switch created, deepest first: only real folders, only when empty
    for d in j.created.iter().rev() {
        let p = rel_path(&l.current(), d);
        match here(&p, &mut errors) {
            Some(true) if !inside(&l.current(), &p) || !plain_dir(&p) => errors.push(format!("{d} is not a plain folder")),
            Some(true) => {
                if let Err(e) = std::fs::remove_dir(&p) {
                    errors.push(format!("remove the new folder {d}: {e}"));
                }
            }
            _ => {}
        }
    }
    // the end state, from the record: every file exactly as the old version had it, nothing left in backup\
    for i in &j.incoming {
        let (cur, bak) = (rel_path(&l.current(), &i.file.rel), rel_path(&l.backup(), &i.file.rel));
        if here(&bak, &mut errors) == Some(true) {
            errors.push(format!("the previous {} is still in backup\\", i.file.rel));
        }
        match (&i.old, here(&cur, &mut errors)) {
            (_, None) => {}
            (Some(_), Some(false)) => errors.push(format!("the previous {} is not back", i.file.rel)),
            (Some(old), Some(true)) => match is_file_of(&cur, old) {
                Ok(true) => {}
                Ok(false) => errors.push(format!("{} is not the previous version's file", i.file.rel)),
                Err(e) => errors.push(e),
            },
            (None, Some(true)) => errors.push(format!("the new {} is still in place", i.file.rel)),
            (None, Some(false)) => {}
        }
    }
    for rel in &j.outgoing {
        if here(&rel_path(&l.backup(), rel), &mut errors) == Some(true) {
            errors.push(format!("the previous {rel} is still in backup\\"));
        }
        if !std::fs::symlink_metadata(rel_path(&l.current(), rel)).is_ok_and(|m| m.is_file()) {
            errors.push(format!("the previous {rel} is not back"));
        }
    }
    errors.dedup();
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    std::fs::remove_file(l.journal()).map_err(|e| format!("everything is back, but the switch record stays ({e})"))?;
    let _ = std::fs::remove_file(l.undoing());
    let _ = std::fs::remove_dir_all(l.backup());
    mark_switching_here(l, false);
    eprintln!("[update] in-place switch undone: {} holds {} again", l.current().display(), bundle_version(&l.current()));
    Ok(())
}

/// What a start found and did about an unsettled in-place switch.
#[derive(Debug, PartialEq, Eq)]
pub enum Recovery {
    /// nothing to do (no record, or this start is the new version of a finished switch)
    Nothing,
    /// the old files are back and this start is the old program: go on
    Undone,
    /// the old files are back but THIS process is the new program (an undo had stopped half way): start
    /// current\tinhat.exe (the old program again) and quit
    UndoneRestartOld,
    /// the undo didn't finish. `stop` = this process is (or may be) the new program next to the previous version's
    /// files: it must not go on (third review); the next start tries again.
    Failed { why: String, stop: bool },
}

/// At a start of the installed app (after the single-instance lock, so never next to a running switch). An unsettled
/// in-place switch is undone when (a) its new tinhat.exe is still in staging\bundle: the switch was cut off before
/// the program moved, so this start is the old program; or (b) staging\UNDOING exists: an undo stopped half way, so
/// it is finished whichever program this start is. Otherwise the new tinhat.exe is in place and this start is the new
/// version of a finished switch: nothing is undone (it settles when its window is up). Which program this start is:
/// the file at current\tinhat.exe (nothing moves it between this process's start and here: the switching process
/// held the lock until it quit). Known limit: a cut that left a changed static-import DLL half-swapped can stop the
/// old program in the loader, before this runs (UPDATE_PATH §3.7).
pub fn recover_interrupted(l: &Layout) -> Recovery {
    let _t = txn();
    if !switch_unsettled(l) {
        return Recovery::Nothing;
    }
    let undoing = l.undoing().exists();
    if !undoing && !l.staged().join(EXE).is_file() {
        return Recovery::Nothing;
    }
    let j = match read_journal(l) {
        Ok(Some(j)) => j,
        Ok(None) => return Recovery::Nothing,
        Err(e) => return Recovery::Failed { why: format!("the switch record: {e}"), stop: undoing },
    };
    let new_exe = j.incoming.iter().find(|i| is_exe(&i.file.rel)).map(|i| is_file_of(&l.current().join(EXE), &i.file));
    let new_running = match new_exe {
        Some(Ok(b)) => b,
        // the program file really isn't there (NotFound only): this start can't be it (final review)
        Some(Err(_)) if present(&l.current().join(EXE)) == Ok(false) => false,
        Some(Err(e)) => return Recovery::Failed { why: format!("which program is this: {e}"), stop: true },
        None => false,
    };
    match undo_locked(l, &Retry::default()) {
        Ok(()) if new_running => Recovery::UndoneRestartOld,
        Ok(()) => Recovery::Undone,
        Err(why) => Recovery::Failed { why, stop: new_running },
    }
}

/// Throw away whatever is staged (a refused probe, a failed download). Best effort.
pub fn discard_staging(l: &Layout) {
    if switch_unsettled(l) {
        eprintln!("[update] staging\\ kept: it holds an unfinished switch's record");
        return;
    }
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
/// 12.1.17: not while an in-place switch is unsettled (its record in staging\ is what puts the old files back if the new
/// version doesn't take over); that is settled once the new version's window is up ([`sweep_backup`]).
pub fn sweep_staging(l: &Layout) {
    let _t = txn();
    if switch_unsettled(l) {
        eprintln!("[platform] staging\\ kept: the switch to the new version settles once its window is up");
        return;
    }
    sweep_dirs(&[l.staging()]);
}

/// The main window is up: the previous version in backup\ is no longer needed. 12.1.17: an in-place switch is settled
/// too (its record goes FIRST, so a stopped sweep can never undo a version that runs), then staging\. Never in the
/// process that started the switch (the old instance), never while an undo is under way.
pub fn sweep_backup(l: &Layout) {
    let _t = txn();
    if switching_here(l) {
        eprintln!("[platform] backup\\ kept: this instance is switching to an update");
        return;
    }
    if switch_unsettled(l) {
        if settle_switch(l) {
            sweep_dirs(&[l.backup(), l.staging()]);
        }
        return;
    }
    sweep_dirs(&[l.backup()]);
}

/// Both (the headless verify phase: it runs in the new version once the old one has handed over).
pub fn sweep(l: &Layout) {
    let _t = txn();
    if switching_here(l) || !settle_switch(l) {
        return;
    }
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

/// The switch. 12.1.17: a download with a plan (unchanged files kept) switches IN PLACE ([`switch_in_place`]); one
/// without (nothing unchanged, or staged by an older version) the old way: current\ -> backup\, staging\bundle ->
/// current\, and on a failed second move the first is undone.
pub fn switch(l: &Layout, r: &Retry) -> Result<(), String> {
    if !l.current().join(EXE).is_file() {
        return Err(format!("no installed app in {}", l.current().display()));
    }
    if !l.staged().join(EXE).is_file() {
        return Err("nothing staged".to_string());
    }
    if switch_unsettled(l) {
        return Err("an earlier switch isn't settled".to_string());
    }
    let plan = read_plan(l).map_err(|e| format!("the update's file list: {e}"))?;
    if l.backup().exists() {
        std::fs::remove_dir_all(l.backup()).map_err(|e| format!("an earlier backup is still in use ({e})"))?;
    }
    if let Some(plan) = plan {
        let to = bundle_version(&l.staged());
        if !plan.fits(l, &to) {
            return Err("the installed app changed since the update was downloaded".to_string());
        }
        return switch_in_place(l, &plan, r, None);
    }
    // the whole-folder switch: the same lock + mark, so this instance's own late backup\ sweep can't take the old
    // version away while the new one is supervised (third review)
    let _t = txn();
    mark_switching_here(l, true);
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
/// 12.1.17, in place: the moved files go back (the unchanged ones were never touched), then staging is deleted.
pub fn roll_back(l: &Layout, r: &Retry) -> Result<(), String> {
    if switch_unsettled(l) {
        undo_in_place(l, r)?;
        discard_staging(l);
        return Ok(());
    }
    if !l.backup().join(EXE).is_file() {
        return Err(format!("no previous version in {}", l.backup().display()));
    }
    let _ = std::fs::create_dir_all(l.staging());
    if l.refused().exists() {
        let _ = std::fs::remove_dir_all(l.refused());
    }
    rename_retry(&l.current(), &l.refused(), r).map_err(|e| format!("move the refused version out: {e}"))?;
    rename_retry(&l.backup(), &l.current(), r).map_err(|e| format!("put the previous version back: {e}"))?;
    mark_switching_here(l, false);
    discard_staging(l);
    Ok(())
}

/// A switch that started fine but was called off (a call began meanwhile): the new bundle goes back to staging (still
/// READY, so the next restart switches again) and the old one back to current\.
pub fn unswitch(l: &Layout, r: &Retry) -> Result<(), String> {
    if switch_unsettled(l) {
        return undo_in_place(l, r); // the new files go back to staging\bundle; READY + the plan stay
    }
    rename_retry(&l.current(), &l.staged(), r).map_err(|e| format!("move the update back to staging: {e}"))?;
    rename_retry(&l.backup(), &l.current(), r).map_err(|e| format!("put the previous version back: {e}"))?;
    mark_switching_here(l, false);
    Ok(())
}

/// After any failed attempt: current\ must hold the version it held before (`previous` = its VERSION then), with its
/// exe. If current\ is gone and backup\ holds a bundle, that one goes back first (Astra review: a failed second move
/// must never leave the shortcuts with nothing). Err = the shortcuts may not start the old version: say so plainly.
pub fn kept_in_place(l: &Layout, previous: &str, r: &Retry) -> Result<(), String> {
    if switch_unsettled(l) {
        // 12.1.17: an in-place switch that wasn't undone yet (or only partly): finish putting the old files back. A
        // failure here is the answer (VERSION + exe alone don't prove a whole old version: second review).
        undo_in_place(l, r).map_err(|e| format!("not every previous file is back: {e}"))?;
    }
    if !l.current().exists() && l.backup().join(EXE).is_file() {
        rename_retry(&l.backup(), &l.current(), r).map_err(|e| format!("current\\ is missing and putting backup\\ back failed: {e}"))?;
    }
    let now = bundle_version(&l.current());
    if l.current().join(EXE).is_file() && now == previous {
        mark_switching_here(l, false);
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
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg(PROBE_ARG)
            .current_dir(&l.root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .env_remove("TINHAT_RELAUNCH")
            .env_remove(crate::relaunch::ACK_ENV)
            .env_remove("TINHAT_UPDATE_TEST")
            .creation_flags(CREATE_NO_WINDOW);
        // 12.1.17: the staged bundle holds only the files that changed (plan_unchanged); the program finds the DLLs it
        // keeps in current\ (the loader searches PATH after the program's own folder). The probe maps only the new
        // files, which are the only ones Windows hasn't judged on this PC yet.
        if let Some(path) = probe_path(l) {
            cmd.env("PATH", path);
        }
        // no "bad image" / "unable to start" dialog from the probe (the mode is inherited at CreateProcess)
        let spawned = with_error_mode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX, || cmd.spawn());
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

    /// A test's install root under the temp folder, deleted when the test ends, passed or failed (Drop also runs on
    /// a panic's unwind): a desktop suite run used to leave ~170 folders per run in /tmp (12.1.17).
    struct Tmp(Layout);
    impl std::ops::Deref for Tmp {
        type Target = Layout;
        fn deref(&self) -> &Layout {
            &self.0
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0.root);
        }
    }
    fn tmp(name: &str) -> Tmp {
        let root = std::env::temp_dir().join(format!("tinhat-wus-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Tmp(Layout::new(root))
    }
    fn bundle(dir: &Path, ver: &str) {
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::write(dir.join(EXE), format!("MZ exe {ver}")).unwrap();
        std::fs::write(dir.join("lib").join("a.dll"), format!("MZ dll {ver}")).unwrap();
        std::fs::write(dir.join("VERSION"), ver).unwrap();
    }
    /// installed 1.0 in current\ + a downloaded 2.0 staged and marked ready (the download step)
    fn installed_and_staged(name: &str) -> Tmp {
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

    // ---- 12.1.17: unchanged files are never written again (Smart App Control) ----

    /// The file's identity on disk: a moved/renamed file keeps it, a fresh copy gets a new one (the property Smart App
    /// Control's cached verdict lives with).
    #[cfg(unix)]
    fn ident(p: &Path) -> (u64, u64) {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::metadata(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        (m.dev(), m.ino())
    }
    /// Windows: the volume serial + the NTFS file index (what a rename keeps and a fresh copy doesn't).
    #[cfg(windows)]
    fn ident(p: &Path) -> (u64, u64) {
        use std::os::windows::io::AsRawHandle;
        #[repr(C)]
        #[derive(Default)]
        struct Info {
            attrs: u32,
            times: [u32; 6],
            serial: u32,
            size_hi: u32,
            size_lo: u32,
            links: u32,
            index_hi: u32,
            index_lo: u32,
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn GetFileInformationByHandle(h: *mut std::ffi::c_void, info: *mut Info) -> i32;
        }
        let f = std::fs::File::open(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        let mut i = Info::default();
        assert!(unsafe { GetFileInformationByHandle(f.as_raw_handle() as _, &mut i) } != 0, "{}", p.display());
        (i.serial as u64, ((i.index_hi as u64) << 32) | i.index_lo as u64)
    }
    fn put(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }
    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    }
    const KEPT: [&str; 3] = ["Qt6Core.dll", "lib/same.dll", "qml/org/kde/kirigami/KirigamiDialogs.dll"];
    /// installed 1.0 (two unchanged DLLs + one in a deep folder, one that changes with the SAME size, one the new
    /// version drops) + a 2.0 download (+ one new DLL in a new folder), staged the way stage_update_zip does it (minus
    /// the Windows probe) and marked ready.
    fn with_unchanged(name: &str) -> Tmp {
        let l = tmp(name);
        let c = l.current();
        put(&c.join(EXE), "MZ exe 1.0");
        put(&c.join("VERSION"), "1.0");
        for k in KEPT {
            put(&rel_path(&c, k), &format!("MZ the same bytes in both: {k}"));
        }
        put(&c.join("lib").join("changed.dll"), "MZ changed 1.0");
        put(&c.join("lib").join("gone.dll"), "MZ gone in 2.0");
        let n = prepare_staging(&l).unwrap().join("tinhat-2.0-win64");
        put(&n.join(EXE), "MZ exe 2.0");
        put(&n.join("VERSION"), "2.0");
        for k in KEPT {
            put(&rel_path(&n, k), &format!("MZ the same bytes in both: {k}"));
        }
        put(&n.join("lib").join("changed.dll"), "MZ changed 2.0");
        put(&n.join("plugins").join("new").join("new.dll"), "MZ new in 2.0");
        adopt_unpacked(&l, "2.0").unwrap();
        plan_unchanged(&l).unwrap(); // (the count is checked in the_download_drops_only_byte_identical_copies)
        mark_ready(&l, "2.0").unwrap();
        l
    }
    fn idents(l: &Layout) -> Vec<(u64, u64)> {
        KEPT.iter().map(|k| ident(&rel_path(&l.current(), k))).collect()
    }
    fn assert_old(l: &Layout) {
        let c = l.current();
        assert_eq!(read(&c.join(EXE)), "MZ exe 1.0");
        assert_eq!(bundle_version(&c), "1.0");
        assert_eq!(read(&c.join("lib").join("changed.dll")), "MZ changed 1.0");
        assert_eq!(read(&c.join("lib").join("gone.dll")), "MZ gone in 2.0");
        assert!(!c.join("plugins").exists(), "the new version's new folder is gone again");
        for k in KEPT {
            assert_eq!(read(&rel_path(&c, k)), format!("MZ the same bytes in both: {k}"));
        }
        assert!(!switch_unsettled(l));
    }
    fn assert_new(l: &Layout) {
        let c = l.current();
        assert_eq!(read(&c.join(EXE)), "MZ exe 2.0");
        assert_eq!(bundle_version(&c), "2.0");
        assert_eq!(read(&c.join("lib").join("changed.dll")), "MZ changed 2.0");
        assert_eq!(read(&c.join("plugins").join("new").join("new.dll")), "MZ new in 2.0");
        assert!(!c.join("lib").join("gone.dll").exists(), "a file the new version dropped is out of current\\");
        for k in KEPT {
            assert_eq!(read(&rel_path(&c, k)), format!("MZ the same bytes in both: {k}"));
        }
    }

    /// THE 12.1.17 rule (failing-first: up to 12.1.16 the new version ran on fresh copies of every file): after a whole
    /// update (download, switch, start), the unchanged files in current\ are the very files that were installed.
    #[test]
    fn an_update_keeps_the_installed_copies_of_unchanged_files() {
        let l = with_unchanged("keep");
        let before = idents(&l);
        let (f, _) = fake(None);
        let h = start(&l, |_| Ok(f), || true).unwrap();
        assert!(h.switched && h.version == "2.0");
        assert_new(&l);
        assert_eq!(idents(&l), before, "the installed copies stay in use: not one unchanged file was rewritten or moved");
        // the OLD instance's own late backup\ sweep never settles the switch it started (second review)
        sweep_backup(&l);
        sweep(&l);
        assert!(switch_unsettled(&l) && l.backup().join(EXE).is_file(), "the old instance leaves its switch alone");
        // the new version's start (another process): nothing to undo, the record waits for its window, then settled
        mark_switching_here(&l, false);
        assert_eq!(recover_interrupted(&l), Recovery::Nothing, "the new program is in place: never undone");
        assert_eq!(startup_plan(&l, "2.0", newer), StartupPlan::Sweep);
        sweep_staging(&l);
        assert!(switch_unsettled(&l), "staging\\ (the switch record) stays until the window is up");
        sweep_backup(&l);
        assert!(!switch_unsettled(&l) && !l.backup().exists() && !l.staging().exists());
        assert_new(&l);
        assert_eq!(idents(&l), before);
    }

    #[test]
    fn the_download_drops_only_byte_identical_copies() {
        let l = with_unchanged("dropplan");
        let s = l.staged();
        for k in KEPT {
            assert!(!rel_path(&s, k).exists(), "{k}: no fresh copy is kept");
        }
        assert!(s.join(EXE).is_file() && s.join("VERSION").is_file(), "the program and VERSION always come new");
        assert_eq!(read(&s.join("lib").join("changed.dll")), "MZ changed 2.0", "same size, other bytes: new");
        assert!(s.join("plugins").join("new").join("new.dll").is_file());
        let p = read_plan(&l).unwrap().unwrap();
        assert_eq!((p.from.as_str(), p.to.as_str()), ("1.0", "2.0"));
        let mut rels: Vec<&str> = p.files.iter().map(|k| k.rel.as_str()).collect();
        rels.sort();
        let mut want = KEPT.to_vec();
        want.sort();
        assert_eq!(rels, want);
        let k = &p.files.iter().find(|k| k.rel == "lib/same.dll").unwrap();
        assert_eq!(k.len, "MZ the same bytes in both: lib/same.dll".len() as u64);
        assert_eq!(k.sha256, file_sha256(&l.current().join("lib").join("same.dll")).unwrap());
        assert_eq!(bundle_version(&l.current()), "1.0", "the download never touches current\\");
        assert_eq!(ready_version(&l).as_deref(), Some("2.0"));
        let path = probe_path(&l).unwrap();
        assert_eq!(std::env::split_paths(&path).next(), Some(l.current()), "the probe finds the kept DLLs in current\\");
    }

    #[test]
    fn a_refused_new_version_gets_every_old_file_back() {
        let l = with_unchanged("refused-inplace");
        let before = idents(&l);
        let r = start(&l, |_| Err(std::io::Error::from_raw_os_error(4551)), || false).unwrap_err();
        assert!(matches!(r.refusal, Refusal::Blocked(_)), "{:?}", r.refusal);
        assert!(r.rollback.is_ok(), "{:?}", r.rollback);
        assert_old(&l);
        assert_eq!(idents(&l), before, "the unchanged files were never touched, so the old version keeps them as they were");
        assert!(!l.backup().exists() && !l.staging().exists(), "nothing left half-done");
        // a start that dies in the loader / never answers: the same
        for (i, exits) in [Some((2, 0xC000_0428u32 as i32)), None].into_iter().enumerate() {
            let l = with_unchanged(&format!("refused-inplace-{i}"));
            let before = idents(&l);
            let (f, _) = fake(exits);
            let r = start(&l, |_| Ok(f), || false).unwrap_err();
            assert!(r.rollback.is_ok(), "{:?}", r.rollback);
            assert_old(&l);
            assert_eq!(idents(&l), before);
        }
    }

    #[test]
    fn a_called_off_in_place_switch_stays_ready_and_switches_again() {
        let l = with_unchanged("calloff-inplace");
        let before = idents(&l);
        let (f, _) = fake(None);
        let h = start(&l, |_| Ok(f), || true).unwrap();
        assert!(h.switched);
        unswitch(&l, &FAST).unwrap();
        assert_old(&l);
        assert_eq!(ready_version(&l).as_deref(), Some("2.0"), "the next restart switches again");
        let (f, _) = fake(None);
        start(&l, |_| Ok(f), || true).unwrap();
        assert_new(&l);
        assert_eq!(idents(&l), before);
    }

    /// A switch cut off before every possible move: an error inside it undoes at once; a process that died there (no
    /// undo ran) is undone at the next start of the old program, or left to settle when the new one is in place.
    #[test]
    fn a_switch_cut_off_anywhere_ends_on_one_whole_version() {
        let mut k = 0;
        loop {
            assert!(k < 200, "the switch never completed: every move number failed (the loop must end)");
            // (a) an error at move k: undone before switch() returns
            let l = with_unchanged(&format!("cut-a{k}"));
            let before = idents(&l);
            let plan = read_plan(&l).unwrap().unwrap();
            match switch_in_place(&l, &plan, &FAST, Some(k)) {
                Err(e) => {
                    assert!(e.contains("the installed app is back"), "{e}");
                    assert_old(&l);
                    assert_eq!(idents(&l), before);
                    assert_eq!(ready_version(&l).as_deref(), Some("2.0"));
                }
                Ok(()) => {
                    assert!(k > 3, "the switch has several moves");
                    assert_new(&l);
                    let _ = std::fs::remove_dir_all(&l.root);
                    break;
                }
            }
            let _ = std::fs::remove_dir_all(&l.root);
            // (b) the process died at move k: the next start decides from where tinhat.exe is
            let l = with_unchanged(&format!("cut-b{k}"));
            let before = idents(&l);
            let plan = read_plan(&l).unwrap().unwrap();
            let j = make_journal(&l, &plan).unwrap();
            assert_eq!(j.outgoing, vec!["lib/gone.dll".to_string()]);
            write_journal(&l, &j).unwrap();
            let _ = switch_moves(&l, &j, &FAST, Some(k));
            assert!(switch_unsettled(&l));
            if l.staged().join(EXE).is_file() {
                if !l.current().join(EXE).is_file() {
                    // cut between moving the old program out and the new one in: nothing could start (accepted)
                    assert!(l.backup().join(EXE).is_file());
                    assert!(kept_in_place(&l, "1.0", &FAST).is_ok(), "the old process's own clean-up still restores it");
                } else {
                    assert_eq!(recover_interrupted(&l), Recovery::Undone, "move {k}: the old program starts and puts its files back");
                }
                assert_old(&l);
                assert_eq!(idents(&l), before);
                assert_eq!(ready_version(&l).as_deref(), Some("2.0"), "still ready: the next restart tries again");
                assert_eq!(plan.files.len(), KEPT.len());
            } else {
                assert_eq!(recover_interrupted(&l), Recovery::Nothing, "the new program is in place: it settles at its window");
            }
            let _ = std::fs::remove_dir_all(&l.root);
            k += 1;
        }
    }

    #[test]
    fn a_staged_update_that_no_longer_fits_the_installed_app_is_not_used() {
        // the installed app changed after the download (e.g. Setup put another version there)
        let l = with_unchanged("stale-ver");
        std::fs::write(l.current().join("VERSION"), "1.1").unwrap();
        assert_eq!(ready_version(&l), None);
        assert_eq!(startup_plan(&l, "1.1", newer), StartupPlan::Sweep);
        assert!(switch(&l, &FAST).unwrap_err().contains("changed since"));
        // a kept file is gone or another size
        let l = with_unchanged("stale-file");
        std::fs::remove_file(l.current().join("lib").join("same.dll")).unwrap();
        assert_eq!(ready_version(&l), None);
        let l = with_unchanged("stale-size");
        std::fs::write(l.current().join("Qt6Core.dll"), "MZ").unwrap();
        assert_eq!(ready_version(&l), None);
        // nothing was moved by any of it
        assert!(!switch_unsettled(&l) && !l.backup().exists());
    }

    #[test]
    fn a_damaged_file_list_is_never_used() {
        let l = with_unchanged("badplan");
        let good = read(&l.staging().join(PLAN_FILE));
        for bad in [
            String::from("garbage\n"),
            good.replacen("lib/same.dll", "../outside.dll", 1),
            good.replacen("lib/same.dll", "lib\\same.dll", 1),
            good.replacen("lib/same.dll", "C:/same.dll", 1),
            good.replacen("lib/same.dll", "tinhat.exe", 1),
            good.replacen(" lib/same.dll", "lib/same.dll", 1),
        ] {
            std::fs::write(l.staging().join(PLAN_FILE), &bad).unwrap();
            assert!(read_plan(&l).is_err(), "{bad:?}");
            assert_eq!(ready_version(&l), None, "{bad:?}");
            assert!(switch(&l, &FAST).is_err());
            assert!(!switch_unsettled(&l));
        }
        assert!(!safe_rel("a/./b") && !safe_rel("/a") && !safe_rel("a//b") && !safe_rel("") && safe_rel("qml/org/a.dll"));
    }

    #[test]
    fn nothing_unchanged_means_the_old_whole_folder_switch() {
        let l = tmp("noplan");
        bundle(&l.current(), "1.0");
        bundle(&prepare_staging(&l).unwrap().join("b"), "2.0");
        adopt_unpacked(&l, "2.0").unwrap();
        assert_eq!(plan_unchanged(&l).unwrap(), 0, "every file differs");
        assert!(read_plan(&l).unwrap().is_none() && probe_path(&l).is_none());
        mark_ready(&l, "2.0").unwrap();
        let (f, _) = fake(None);
        start(&l, |_| Ok(f), || true).unwrap();
        assert_eq!(bundle_version(&l.current()), "2.0");
        assert_eq!(bundle_version(&l.backup()), "1.0", "the whole old folder in backup\\, as before");
    }

    /// Second review: an undo run again (e.g. its record couldn't be deleted the first time) never moves an OLD file.
    #[test]
    fn an_undo_run_twice_never_moves_an_old_file() {
        let l = with_unchanged("replay");
        let before = idents(&l);
        let plan = read_plan(&l).unwrap().unwrap();
        let j = make_journal(&l, &plan).unwrap();
        write_journal(&l, &j).unwrap();
        switch_moves(&l, &j, &FAST, None).unwrap();
        assert_new(&l);
        undo_in_place(&l, &FAST).unwrap();
        assert_old(&l);
        let old_exe = ident(&l.current().join(EXE));
        // the record is back (a deletion that failed): the replay finds nothing of the new version in current\
        write_journal(&l, &j).unwrap();
        undo_in_place(&l, &FAST).unwrap();
        assert_old(&l);
        assert_eq!(ident(&l.current().join(EXE)), old_exe, "the old program stayed where it was");
        assert_eq!(idents(&l), before);
        assert!(l.staged().join(EXE).is_file(), "the new program is still staged");
    }

    /// Second review: a file that becomes a folder (or back), a name that changes only in case, or a link anywhere
    /// makes the undo ambiguous: such an update keeps every downloaded file and switches the whole folder, as before.
    #[test]
    fn unusual_bundles_switch_the_whole_folder_as_before() {
        let mk = |name: &str, f: &dyn Fn(&Layout, &Path)| {
            let l = tmp(name);
            bundle(&l.current(), "1.0");
            put(&l.current().join("lib").join("same.dll"), "MZ same");
            put(&l.current().join("assets"), "MZ assets file");
            let n = prepare_staging(&l).unwrap().join("b");
            bundle(&n, "2.0");
            put(&n.join("lib").join("same.dll"), "MZ same");
            f(&*l, &n);
            adopt_unpacked(&l, "2.0").unwrap();
            l
        };
        let file_to_folder = mk("u-f2d", &|_, n| put(&n.join("assets").join("data.dll"), "MZ assets folder"));
        let case_only = mk("u-case", &|_, n| {
            put(&n.join("assets"), "MZ assets file");
            std::fs::remove_file(n.join("lib").join("same.dll")).unwrap();
            put(&n.join("lib").join("SAME.dll"), "MZ same");
        });
        for l in [&file_to_folder, &case_only] {
            assert_eq!(plan_unchanged(l).unwrap(), 0, "{}", l.root.display());
            assert!(read_plan(l).unwrap().is_none() && l.staged().join("lib").is_dir(), "nothing dropped");
            mark_ready(l, "2.0").unwrap();
            let (f, _) = fake(None);
            start(l, |_| Ok(f), || true).unwrap();
            assert_eq!(bundle_version(&l.current()), "2.0");
            assert_eq!(bundle_version(&l.backup()), "1.0", "the whole old folder in backup\\");
        }
        #[cfg(unix)]
        {
            let linked = mk("u-link", &|l, n| {
                put(&n.join("assets"), "MZ assets file");
                std::os::unix::fs::symlink(&l.root, l.current().join("lnk")).unwrap();
            });
            assert_eq!(plan_unchanged(&linked).unwrap(), 0, "a link in the installed app");
        }
        // a conflict that appears AFTER the download: the switch refuses and moves nothing
        let l = with_unchanged("u-late");
        put(&l.current().join("plugins").join("new"), "MZ a file where 2.0 has a folder");
        let e = switch(&l, &FAST).unwrap_err();
        assert!(e.contains("changes between a file and a folder"), "{e}");
        assert!(!switch_unsettled(&l) && !l.backup().exists());
        assert_eq!(read(&l.current().join(EXE)), "MZ exe 1.0");
        assert_eq!(key("lib/ς.dll"), key("LIB/Σ.DLL"), "NTFS-style 1:1 uppercase keys");
        assert_eq!(key("Lib/Ä.DLL"), key("lib/ä.dll"));
    }

    /// Second review: a file the undo can't read is never guessed about; nothing is deleted and the record stays.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_file_stops_the_undo_without_losing_anything() {
        use std::os::unix::fs::PermissionsExt;
        let l = with_unchanged("unreadable");
        let plan = read_plan(&l).unwrap().unwrap();
        let j = make_journal(&l, &plan).unwrap();
        write_journal(&l, &j).unwrap();
        switch_moves(&l, &j, &FAST, None).unwrap();
        let p = l.current().join("lib").join("changed.dll");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&p).is_ok() {
            return; // running as root: permissions don't stop a read here
        }
        let e = undo_in_place(&l, &FAST).unwrap_err();
        assert!(e.contains("lib/changed.dll"), "{e}");
        assert!(switch_unsettled(&l) && l.undoing().exists() && l.backup().join("lib").join("changed.dll").is_file());
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!sweep_backup_settles(&l), "no settle while an undo is under way");
        undo_in_place(&l, &FAST).unwrap();
        assert_old(&l);
    }

    fn sweep_backup_settles(l: &Layout) -> bool {
        mark_switching_here(l, false);
        sweep_backup(l);
        !switch_unsettled(l)
    }

    /// Second review: an undo that stopped half way (the new program couldn't be moved out) is finished at the next
    /// start even when that start IS the new program; it then hands over to the old one.
    #[test]
    fn an_undo_stopped_half_way_is_finished_by_whichever_program_starts() {
        let l = with_unchanged("half-undo");
        let before = idents(&l);
        let plan = read_plan(&l).unwrap().unwrap();
        let j = make_journal(&l, &plan).unwrap();
        write_journal(&l, &j).unwrap();
        switch_moves(&l, &j, &FAST, None).unwrap();
        // something sits where the new program must go back: its move out fails, the rest goes back
        std::fs::create_dir_all(l.staged().join(EXE).join("blocker")).unwrap();
        assert!(undo_in_place(&l, &FAST).is_err());
        assert!(l.undoing().exists() && switch_unsettled(&l));
        assert_eq!(read(&l.current().join(EXE)), "MZ exe 2.0", "the new program is still in place");
        assert!(!sweep_backup_settles(&l), "the new program's window must not settle a half undo");
        std::fs::remove_dir_all(l.staged().join(EXE)).unwrap();
        assert_eq!(recover_interrupted(&l), Recovery::UndoneRestartOld);
        assert_old(&l);
        assert_eq!(idents(&l), before);
        assert!(!l.undoing().exists());
    }

    /// Second review: a kept file that changed since the download (same size, other bytes) stops the switch.
    #[test]
    fn a_kept_file_changed_since_the_download_stops_the_switch() {
        let l = with_unchanged("kept-changed");
        let p = l.current().join("lib").join("same.dll");
        let n = read(&p).len();
        std::fs::write(&p, "X".repeat(n)).unwrap();
        assert_eq!(ready_version(&l).as_deref(), Some("2.0"), "the cheap check (sizes) still passes");
        let e = switch(&l, &FAST).unwrap_err();
        assert!(e.contains("changed since") && e.contains("lib/same.dll"), "{e}");
        assert!(!switch_unsettled(&l) && !l.backup().exists());
        assert_eq!(read(&l.current().join(EXE)), "MZ exe 1.0");
    }

    /// Second review: nothing deletes an unsettled switch's record (a new download, a discard, a sweep elsewhere).
    #[test]
    fn an_unsettled_switch_record_survives_everything_but_settle_and_undo() {
        let l = with_unchanged("guard");
        let (f, _) = fake(None);
        start(&l, |_| Ok(f), || true).unwrap();
        assert!(switch_unsettled(&l));
        // launch-gate 11-19: the words a person sees for it (the update error line)
        assert_eq!(
            prepare_staging(&l).unwrap_err(),
            "an earlier update isn't finished yet; restart Tin Hat and try again",
            "no new download over an unsettled switch"
        );
        discard_staging(&l);
        sweep_staging(&l);
        assert!(switch_unsettled(&l) && l.backup().join(EXE).is_file());
        // the old instance calls it off: everything back, record gone, the update still ready
        unswitch(&l, &FAST).unwrap();
        assert_old(&l);
        assert_eq!(ready_version(&l).as_deref(), Some("2.0"));
    }

    /// Third review: an empty folder the new version ships is created by the switch and removed by the undo.
    #[test]
    fn an_empty_new_folder_comes_and_goes_with_the_switch() {
        let l = with_unchanged("emptydir");
        std::fs::create_dir_all(l.staged().join("cache").join("templates")).unwrap();
        let plan = read_plan(&l).unwrap().unwrap();
        let j = make_journal(&l, &plan).unwrap();
        assert!(j.created.contains(&"cache/templates".to_string()), "{:?}", j.created);
        write_journal(&l, &j).unwrap();
        switch_moves(&l, &j, &FAST, None).unwrap();
        assert!(l.current().join("cache").join("templates").is_dir());
        undo_in_place(&l, &FAST).unwrap();
        assert!(!l.current().join("cache").exists());
        assert_old(&l);
    }

    /// Third review: something new where a removed file must go back is never overwritten; the record stays.
    #[test]
    fn an_occupied_path_keeps_the_previous_file_and_the_record() {
        let l = with_unchanged("occupied");
        let plan = read_plan(&l).unwrap().unwrap();
        let j = make_journal(&l, &plan).unwrap();
        write_journal(&l, &j).unwrap();
        switch_moves(&l, &j, &FAST, None).unwrap();
        put(&l.current().join("lib").join("gone.dll"), "MZ something else");
        let e = undo_in_place(&l, &FAST).unwrap_err();
        assert!(e.contains("lib/gone.dll"), "{e}");
        assert_eq!(read(&l.backup().join("lib").join("gone.dll")), "MZ gone in 2.0", "the previous file is kept");
        assert!(switch_unsettled(&l) && l.undoing().exists());
        std::fs::remove_file(l.current().join("lib").join("gone.dll")).unwrap();
        undo_in_place(&l, &FAST).unwrap();
        assert_old(&l);
    }

    #[test]
    fn plan_and_record_paths_stay_inside_the_app_folders() {
        let l = with_unchanged("inside");
        assert!(inside(&l.current(), &l.current().join("lib").join("x.dll")));
        assert!(!inside(&l.current(), &l.root.join("elsewhere.dll")));
        {
            // a linked folder (Windows: a junction, which needs no privilege) inside current\ never leads a move out
            #[cfg(unix)]
            std::os::unix::fs::symlink(&l.root, l.current().join("linked")).unwrap();
            #[cfg(windows)]
            assert!(std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(l.current().join("linked"))
                .arg(&l.root)
                .status()
                .unwrap()
                .success());
            assert!(!inside(&l.current(), &l.current().join("linked").join("x.dll")));
            let e = move_file(&l, &l.current().join("linked").join("x.dll"), &l.backup().join("x.dll"), &FAST).unwrap_err();
            assert!(e.contains("not a plain path"), "{e}");
        }
        assert_eq!(key("Lib/Ä.DLL"), key("lib/ä.dll"), "Windows-style case folding beyond ASCII");
    }
}
