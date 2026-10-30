//! platform — OS-specific glue in one place (windows/PLATFORM_AUDIT.md layout):
//! per-user data paths, `file://` URL building, portable-bundle runtime env, and the
//! Windows self-update folder swap (windows/UPDATE_PATH.md §3).
//!
//! Ground rule of the port: the Linux bodies here are byte-identical in behavior to the
//! historical inline code they replaced — only Windows gains new behavior.

use std::path::{Path, PathBuf};

/// Per-user data base dir — the XDG data home (Linux) / `%LOCALAPPDATA%` (Windows)
/// analogue. Linux: exactly the historical `$XDG_DATA_HOME` → `$HOME/.local/share`
/// resolution (same empty-var handling), so existing installs keep their data.
#[cfg(not(windows))]
pub(crate) fn data_base_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        })
}

/// Windows: `%LOCALAPPDATA%` (per-user, ACL'd, always writable — UPDATE_PATH.md §1).
/// The historical code fell back to `HOME/.local/share`; HOME is unset on Windows, so
/// data silently landed in a CWD-RELATIVE `.local\share\` (observed live 2026-08-08:
/// session/store/avatars inside the repo checkout). `%TEMP%` fallback is a last resort
/// that keeps the app bootable on a broken profile. The rule itself is
/// [`windows_data_base`] (pure, unit-tested on Linux).
#[cfg(windows)]
pub(crate) fn data_base_dir() -> PathBuf {
    windows_data_base(&env_var, &std::env::temp_dir())
}

/// The app's own data root: `<base>/TinHat` — session.json, sqlite stores, caches,
/// prefs. On Windows this sits BESIDE the updater's `current\`/`staging\`/`backup\`
/// bundle dirs (UPDATE_PATH.md §1), so a bundle swap never touches login/keys/history.
pub(crate) fn app_data_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        #[cfg(windows)]
        {
            // The Windows folder-swap updater is rooted here too (UPDATE_PATH.md §1): the bundle
            // lives in `current\`, beside the data.
            data_base_dir().join(WINDOWS_ROOT_NAME)
        }
        #[cfg(not(windows))]
        {
            data_base_dir().join("TinHat")
        }
    })
    .clone()
}

// ── The per-user directories are fixed before main() touches a file ────────────────────
//
// Every `$XDG_CONFIG_HOME, else $HOME/.config` reader (settings.rs' prefs overlay and the
// safe-mode start marker, the streamer's device/AEC/share prefs, notify, logship, …) and
// every `$XDG_CACHE_HOME, else $HOME/.cache` reader (room.rs, linkpreview.rs) assumes a
// HOME. Windows has none, so until those two variables are exported, the paths are
// RELATIVE to the folder Tin Hat was started from. init_bundle_env() exported them — but
// only after boot.rs had already read and armed the safe-mode marker under
// `<launch folder>\.config\`, while the "reached the main window" write went under
// `<data root>\config\`. The first file never got its "ok", so every second start from the
// same place ran in safe mode.
//
// Now `init_dirs()` runs first in main() (after the timing anchor), before any file is read
// or written, and the rule is the pure `resolve_dirs()`, so what both platforms resolve to is
// pinned by unit tests that run on Linux. On Windows a directory variable only counts when it
// is set, non-empty and absolute: a relative one moves with the working folder, which is the
// same split again (the marker armed under one folder, cleared under another after
// early_startup's chdir) - and the XDG base-dir spec says relative values must be ignored.

/// The Windows data root's folder name under `%LOCALAPPDATA%` (also the updater root — see
/// `app_data_dir`). Not `TinHat`: that is the cache folder (`cache_base_dir` callers join
/// `TinHat/...`), which the installer's Repair and Uninstall clear wholesale.
const WINDOWS_ROOT_NAME: &str = "TinHatApp";

/// Which platform's directory rules apply — data, so the Windows rule is testable on Linux.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DirRules {
    /// Linux / Flatpak: the historical XDG fallbacks; nothing is exported.
    Unix,
    /// Windows: HOME is unset, so XDG_CONFIG_HOME / XDG_CACHE_HOME are exported into the
    /// data root before anything reads them (over an unset, empty or relative value).
    Windows,
}

impl DirRules {
    /// The rules of the platform this binary was built for.
    pub(crate) const HOST: DirRules = if cfg!(windows) { DirRules::Windows } else { DirRules::Unix };
}

/// What one environment resolves to (see [`resolve_dirs`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DirPlan {
    /// Where every `$XDG_CONFIG_HOME, else $HOME/.config` reader lands (prefs overlay, the
    /// safe-mode start marker, the streamer's settings).
    pub config_home: PathBuf,
    /// Where every `$XDG_CACHE_HOME, else $HOME/.cache` reader lands.
    pub cache_home: PathBuf,
    /// Variables to export before anything reads them, in order. Always empty on Unix.
    pub exports: Vec<(&'static str, PathBuf)>,
}

/// The process environment, in the shape the pure resolvers take.
pub(crate) fn env_var(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(name)
}

/// A variable that is set and non-empty (the Linux/Flatpak readers' historical rule).
fn non_empty(v: Option<std::ffi::OsString>) -> Option<PathBuf> {
    v.filter(|s| !s.is_empty()).map(PathBuf::from)
}

/// Pure: is `p` absolute by WINDOWS rules — a drive letter with a root (`C:\x`, `C:/x`), two
/// leading separators (UNC `\\server\share`, `\\?\…`, `\\.\…`) or the NT object-manager prefix
/// `\??\` (backslashes only, as Windows and Rust's std parse it)? Lexical, so the Windows rule is
/// unit-testable on Linux (Path::is_absolute answers for the host OS). `cfg`, `.\cfg`, `\cfg`
/// (the root of whatever drive is current) and `C:cfg` (drive-relative) are not.
pub(crate) fn windows_is_absolute(p: &std::ffi::OsStr) -> bool {
    let b = p.as_encoded_bytes();
    let sep = |c: u8| c == b'\\' || c == b'/';
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && sep(b[2]))
        || (b.len() >= 2 && sep(b[0]) && sep(b[1]))
        || b.starts_with(br"\??\")
}

/// A Windows directory variable worth using: set, non-empty and absolute (see above).
fn windows_usable(v: Option<std::ffi::OsString>) -> Option<PathBuf> {
    v.filter(|s| windows_is_absolute(s)).map(PathBuf::from)
}

/// Pure: the Windows per-user base — `%LOCALAPPDATA%` when set, non-empty and absolute, else
/// `temp` (the data root is `<base>\TinHatApp`, the cache base is `<base>` itself).
pub(crate) fn windows_data_base(var: &dyn Fn(&str) -> Option<std::ffi::OsString>, temp: &Path) -> PathBuf {
    windows_usable(var("LOCALAPPDATA")).unwrap_or_else(|| temp.to_path_buf())
}

/// Pure: the per-user directories under `rules` for the environment `var` (the process
/// env as data; `temp` = the temp dir, Windows' last-resort base).
///
/// * Unix: config = `$XDG_CONFIG_HOME` || `$HOME/.config`, cache = `$XDG_CACHE_HOME` ||
///   `$HOME/.cache` (a set-but-empty variable counts as unset); nothing is exported — the
///   historical behaviour, byte for byte (a missing HOME stays the historical relative
///   `.config`).
/// * Windows: config = `$XDG_CONFIG_HOME` || `<base>\TinHatApp\config`, cache =
///   `$XDG_CACHE_HOME` || `<base>`, where a variable counts only when set, non-empty and
///   absolute. Each fallback is exported (over an empty or relative value), so every plain env
///   reader — they read the variables directly, and not all of them filter an empty value
///   (e2ee_keys.rs) — and every child process sees the same absolute folders. An absolute
///   value the user set is kept.
pub(crate) fn resolve_dirs(
    rules: DirRules,
    var: &dyn Fn(&str) -> Option<std::ffi::OsString>,
    temp: &Path,
) -> DirPlan {
    match rules {
        DirRules::Unix => {
            let config = non_empty(var("XDG_CONFIG_HOME"));
            let cache = non_empty(var("XDG_CACHE_HOME"));
            let home = PathBuf::from(var("HOME").unwrap_or_default());
            DirPlan {
                config_home: config.unwrap_or_else(|| home.join(".config")),
                cache_home: cache.unwrap_or_else(|| home.join(".cache")),
                exports: Vec::new(),
            }
        }
        DirRules::Windows => {
            let config = windows_usable(var("XDG_CONFIG_HOME"));
            let cache = windows_usable(var("XDG_CACHE_HOME"));
            let base = windows_data_base(var, temp);
            let mut exports = Vec::new();
            let config_home = match config {
                Some(p) => p,
                None => {
                    let p = base.join(WINDOWS_ROOT_NAME).join("config");
                    exports.push(("XDG_CONFIG_HOME", p.clone()));
                    p
                }
            };
            let cache_home = match cache {
                Some(p) => p,
                None => {
                    exports.push(("XDG_CACHE_HOME", base.clone()));
                    base
                }
            };
            DirPlan { config_home, cache_home, exports }
        }
    }
}

/// Where `$XDG_CONFIG_HOME, else $HOME/.config` points for this process — the one resolver
/// behind the prefs overlay and the safe-mode start marker (settings.rs). Linux/Flatpak:
/// the historical formula. Windows: the data root's `config\` even if something ran before
/// `init_dirs()`.
pub(crate) fn config_home() -> PathBuf {
    resolve_dirs(DirRules::HOST, &env_var, &std::env::temp_dir()).config_home
}

/// What `init_dirs()` exported (Windows), for the log line init_bundle_env() prints once
/// stderr reaches the log file.
#[cfg(windows)]
static EARLY_EXPORTS: std::sync::OnceLock<Vec<(&'static str, PathBuf)>> = std::sync::OnceLock::new();

/// main()'s FIRST line: fix the per-user directories before any file is read or written
/// (the alternating safe-mode fix). Linux/Flatpak: nothing to do — the historical XDG fallbacks apply unchanged.
#[cfg(not(windows))]
pub(crate) fn init_dirs() {}

/// Windows: export XDG_CONFIG_HOME / XDG_CACHE_HOME into the data root when unset, empty or
/// relative — what init_bundle_env() used to do only after the safe-mode decision.
/// Environment only: no file or folder is touched here (so `--uninstall` and a rejected
/// second start create nothing); init_bundle_env() creates the config folder, as it always
/// did. Idempotent: init_bundle_env() calls it again as a safety net. No output here: stderr
/// is not routed to the log yet (init_bundle_env() logs the result).
#[cfg(windows)]
pub(crate) fn init_dirs() {
    if EARLY_EXPORTS.get().is_some() {
        return;
    }
    let plan = resolve_dirs(DirRules::Windows, &env_var, &std::env::temp_dir());
    for (name, value) in &plan.exports {
        // Top of main(): no other thread exists yet (and Windows' set_var is sound regardless).
        std::env::set_var(name, value);
    }
    let _ = EARLY_EXPORTS.set(plan.exports);
}

// ── 0.8.29: where THIS process's diagnostics actually go ──────────────────────────────
//
// Linux used two hardcoded, uid-less paths in world-writable /tmp: any uid mismatch on
// sticky /tmp made rename()/create() fail silently (no diagnostics at all, or three
// launches shipping the same frozen previous-session log — a field report), and a
// pre-created file let another local user get text uploaded under the victim's Matrix
// id. Logs now live in the per-user state dir, mode 0700, mirroring what Windows already
// does under %LOCALAPPDATA%\TinHatApp\logs.

/// Per-user log dir. Linux: `$XDG_STATE_HOME/tinhat` → `~/.local/state/tinhat`.
#[cfg(not(windows))]
pub(crate) fn log_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state")
        })
        .join("tinhat")
}
#[cfg(windows)]
pub(crate) fn log_dir() -> PathBuf {
    app_data_dir().join("logs")
}

/// The file stderr was ACTUALLY redirected to this run (set by the redirect; `None` =
/// stderr was left alone, e.g. TINHAT_NO_LOG_REDIRECT / headless gates).
fn live_log_cell() -> &'static std::sync::OnceLock<Option<PathBuf>> {
    static C: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    &C
}
pub(crate) fn set_live_log(p: Option<PathBuf>) {
    let _ = live_log_cell().set(p);
}

/// Where this session's diagnostics are being written, or `None` when they are not
/// going to a file. Recorded by the redirect; otherwise (update-relaunch child that
/// inherited the parent's handle, or no redirect) read straight from `/proc/self/fd/2`
/// on Linux so the answer can never be "a constant that happens to be wrong".
pub(crate) fn live_log_path() -> Option<PathBuf> {
    if let Some(v) = live_log_cell().get() {
        return v.clone();
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(target) = std::fs::read_link("/proc/self/fd/2") {
            if target.is_absolute() && target.is_file() {
                return Some(target);
            }
            return None;
        }
    }
    Some(log_dir().join("tinhat.log"))
}

/// The previous session's rotated log (rotation happens at startup on both platforms).
pub(crate) fn old_log_path() -> PathBuf {
    log_dir().join("tinhat.old.log")
}

// ── M10 (security review 2026-10-26): the app's per-user folders are private ─────────────────────────────────────
//
// The data root holds session.json (the access token) and the E2EE store (device keys, room keys, the backup key);
// the cache root holds decrypted timeline snapshots, member lists and decrypted media; the prefs folder holds chat
// drafts. Created at the default umask they were 0755: every local account could read the history and the token.
// Unix: each root is 0700 — created so, and an existing install's roots narrowed at every start (files inside need no
// change: nobody else can walk into the folder). The log folder was already 0700 (0.8.29). Windows: %LOCALAPPDATA% is
// per-user already (PLATFORM_AUDIT.md §11). Not covered here (a later change): a passphrase for the E2EE store kept in
// the OS keyring, so a copy of the folder (a backup, a disk image) is no use without the account's keyring.

/// The roots [`harden_private_dirs`] keeps private: the data root, the cache root, the prefs folder.
#[cfg(unix)]
pub(crate) fn private_roots() -> Vec<PathBuf> {
    vec![app_data_dir(), cache_base_dir().join("TinHat"), config_home().join("tinhat")]
}

/// Create each of [`private_roots`] as 0700 (and any missing parent, as the XDG spec wants them) and narrow an existing
/// one that lets group or others in. Returns one line per root that was changed (or could not be), for the log.
#[cfg(unix)]
pub(crate) fn harden_private_dirs() -> Vec<String> {
    let mut out = Vec::new();
    for d in private_roots() {
        match make_private_dir(&d) {
            Ok(true) => out.push(format!("{}: narrowed to 0700", d.display())),
            Ok(false) => {}
            Err(e) => out.push(format!("{}: couldn't make it private: {e}", d.display())),
        }
    }
    out
}

/// `dir` as a private folder: created 0700 when missing (`Ok(false)`), narrowed to 0700 when group/others had any
/// access (`Ok(true)`), left alone when already private (`Ok(false)`).
#[cfg(unix)]
pub(crate) fn make_private_dir(dir: &Path) -> std::io::Result<bool> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    if !dir.exists() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        // the umask may have narrowed it further than 0700 (never wider): fine either way
        return Ok(false);
    }
    let mode = std::fs::metadata(dir)?.permissions().mode();
    if mode & 0o077 == 0 {
        return Ok(false);
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(true)
}

/// Write `bytes` to `path` as a private file (Unix: created 0600 by the open itself, and an existing file narrowed to
/// 0600 too — no window where the token sits at the default umask). Windows: a plain write (per-user folder), flushed.
pub(crate) fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
        // an existing file keeps its mode across the open: narrow it before the secret goes in
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        // flushed like the Unix branch (assembly-038 review): a sign-in's commit (login_swap.rs) removes the old
        // store's copy right after this, so the new session must be on disk first
        use std::io::Write;
        let mut f = std::fs::File::create(path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(())
    }
}

/// Per-user cache base dir. Linux: the historical `$XDG_CACHE_HOME` → `$HOME/.cache`.
/// Windows: `%LOCALAPPDATA%` — there is no data/cache split, so cache callers (which
/// join their own `Tin Hat/...` subpath) land inside the same app root; caches and
/// data sharing one avatar dir is deduplication, not a conflict (same media-id keys).
#[cfg(not(windows))]
pub(crate) fn cache_base_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache")
        })
}

#[cfg(windows)]
pub(crate) fn cache_base_dir() -> PathBuf {
    data_base_dir()
}

/// The filesystem path behind a `file://` URL produced by [`file_url`] — for the APIs
/// that want a real path rather than a URL (freedesktop notification hints, WinRT toast
/// images). Returns the input unchanged when it isn't a file URL.
///
/// The subtlety is Windows: [`file_url`] emits `file:///C:/...` there on purpose, so
/// stripping only the scheme leaves `/C:/...`, which no Win32 API will open. Drop the
/// slash that precedes a drive letter.
pub(crate) fn path_from_file_url(url: &str) -> String {
    let raw = url.strip_prefix("file://").unwrap_or(url);
    match raw.strip_prefix('/') {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => rest.to_string(),
        _ => raw.to_string(),
    }
}

/// A `file://` URL that QML `Image`/`Audio`/`WebEngine` can actually load.
/// Linux: the historical `format!("file://{path}")` byte-for-byte (absolute unix paths
/// make it valid as-is; changing to percent-encoding would alter long-stable behavior).
#[cfg(not(windows))]
pub(crate) fn file_url(path: &Path) -> String {
    format!("file://{}", path.display())
}

/// Windows: `file://C:\rel\path` (what the naive format produces) is unloadable — the
/// drive letter parses as a URL *host* and backslashes aren't path separators. Build a
/// real `file:///C:/...` URL from an absolutized path. Fallback (UNC oddities etc.):
/// hand-rolled forward-slash form, which QML also accepts.
#[cfg(windows)]
pub(crate) fn file_url(path: &Path) -> String {
    let abs: PathBuf = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map(|d| d.join(path)).unwrap_or_else(|_| path.to_path_buf())
    };
    match url::Url::from_file_path(&abs) {
        Ok(u) => u.to_string(),
        Err(()) => format!("file:///{}", abs.display().to_string().replace('\\', "/")),
    }
}

/// A path string QML can prepend `"file://"` to (Main.qml does exactly that for
/// `assetsPath`/`reportPath`). Unix: the plain absolute path (`/home/...` →
/// `file:///home/...`). Windows: leading slash + forward slashes (`/C:/x/y` →
/// `file:///C:/x/y`) — the naive form would parse the drive letter as a URL host.
#[cfg(not(windows))]
pub(crate) fn qml_file_prefix_path(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(windows)]
pub(crate) fn qml_file_prefix_path(path: &Path) -> String {
    format!("/{}", path.display().to_string().replace('\\', "/"))
}

/// The user's home dir. Unix: `$HOME` (historical semantics, empty default). Windows:
/// `%USERPROFILE%` (HOME is unset there — the historical code degraded to a cwd-relative
/// path, dropping files next to whatever launched the app).
#[cfg(not(windows))]
pub(crate) fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

#[cfg(windows)]
pub(crate) fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(std::env::temp_dir)
}

/// Immediate process exit that skips ALL teardown — the Windows twin of the ship gates'
/// `libc::_exit` (see main.rs "skip C++ static destructors"). `std::process::exit` is
/// ExitProcess: it kills the other threads mid-instruction and THEN runs every DLL's
/// DLL_PROCESS_DETACH under the loader lock — with Qt's D3D11 render thread live that
/// can deadlock or AV *after* a verdict prints, corrupting a PASS exit code into a
/// timeout/0xC0000005. TerminateProcess runs no detach callbacks at all.
#[cfg(windows)]
pub(crate) fn hard_exit(code: i32) -> ! {
    use std::io::Write;
    let _ = std::io::stderr().flush();
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn TerminateProcess(process: isize, exit_code: u32) -> i32;
    }
    unsafe {
        TerminateProcess(GetCurrentProcess(), code as u32);
    }
    unreachable!("TerminateProcess returned");
}

/// Early-startup hook: call once at the very top of `main()` (before gst::init and
/// before QGuiApplication). Linux: point GStreamer's CUDA filters at a bundled NVRTC (below).
#[cfg(not(windows))]
pub(crate) fn early_startup() {
    if std::path::Path::new("/.flatpak-info").exists() {
        use_bundled_nvrtc(std::path::Path::new("/app/lib"));
    }
}

/// 12.1.2: GPU scaling for shares in the Flatpak. The runtime's nvcodec plugin registers cudaupload/cudadownload, but
/// its CUDA *filters* (cudaconvertscale) only when it can dlopen NVIDIA's runtime compiler, libnvrtc, which is part of
/// the CUDA toolkit, not the driver, so no Flatpak runtime or GL extension carries it. Without it every Linux share took
/// the CPU convert + lanczos path: 5K shares published 32-37 fps under a 60 cap while NVENC sat at 6-8 ms a frame
/// (STREAM-QUALITY-FINDINGS.md). The manifest bundles the redistributable libnvrtc into /app/lib; this tells nvcodec
/// where it is and gives GStreamer a registry file of its own, because the plugin's features are cached at scan time
/// and the runtime's plugin file never changes, so the old cache (scanned without NVRTC) would be kept forever.
#[cfg(not(windows))]
fn use_bundled_nvrtc(lib_dir: &std::path::Path) {
    // set_var is safe here: early_startup runs at the top of the synchronous main(), before any runtime, logger or
    // worker thread exists (the Windows arm sets its GStreamer env at the same point).
    if std::env::var_os("GST_CUDA_NVRTC_LIBNAME").is_some() {
        return;
    }
    let Some(nvrtc) = bundled_nvrtc(lib_dir) else { return };
    std::env::set_var("GST_CUDA_NVRTC_LIBNAME", &nvrtc);
    // GST_REGISTRY_1_0 wins over GST_REGISTRY in GStreamer: a user who set either keeps theirs.
    if std::env::var_os("GST_REGISTRY").is_none() && std::env::var_os("GST_REGISTRY_1_0").is_none() {
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")));
        if let Some(cache) = cache {
            let reg = cache.join("gstreamer-1.0").join(nvrtc_registry_name(&nvrtc, nvidia_driver_version().as_deref()));
            std::env::set_var("GST_REGISTRY", reg);
        }
    }
}

/// The registry file for a bundled NVRTC: keyed by the real library file (libnvrtc.so.12 -> libnvrtc.so.12.9.86, so
/// a new NVRTC in an update rescans) and the NVIDIA driver version (nvcodec registers nothing CUDA without a driver and
/// caches that, so installing or updating the driver must rescan too). Neither changes the runtime's plugin file,
/// which is all GStreamer itself watches.
#[cfg(not(windows))]
fn nvrtc_registry_name(nvrtc: &std::path::Path, driver: Option<&str>) -> String {
    let real = std::fs::canonicalize(nvrtc).unwrap_or_else(|_| nvrtc.to_path_buf());
    let lib = real.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let drv: String = driver.unwrap_or("nodriver").chars().filter(|c| c.is_ascii_alphanumeric() || *c == '.').collect();
    format!("registry.{lib}.nv-{drv}.{}.bin", std::env::consts::ARCH)
}

/// The loaded NVIDIA kernel driver's version ("595.45.04"), from /proc (visible inside the Flatpak); None without one.
#[cfg(not(windows))]
fn nvidia_driver_version() -> Option<String> {
    let s = std::fs::read_to_string("/proc/driver/nvidia/version").ok()?;
    // "NVRM version: NVIDIA UNIX x86_64 Kernel Module  595.45.04  <date>"
    s.lines()
        .next()?
        .split_whitespace()
        .find(|w| w.contains('.') && w.chars().all(|c| c.is_ascii_digit() || c == '.'))
        .map(str::to_owned)
}

/// The bundled NVRTC library in `lib_dir`: `libnvrtc.so.<major>` resolving to a real file (the builtins library is not
/// it); the highest major wins, compared as a number.
#[cfg(not(windows))]
fn bundled_nvrtc(lib_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    std::fs::read_dir(lib_dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter_map(|p| {
            let major: u32 = p.file_name()?.to_str()?.strip_prefix("libnvrtc.so.")?.parse().ok()?;
            p.is_file().then_some((major, p)) // is_file follows the symlink: a dangling link or a directory is not it
        })
        .max_by_key(|(major, _)| *major)
        .map(|(_, p)| p)
}

#[cfg(all(test, not(windows)))]
mod nvrtc_tests {
    #[test]
    fn finds_the_bundled_nvrtc_not_the_builtins() {
        let dir = std::env::temp_dir().join(format!("tinhat-nvrtc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(super::bundled_nvrtc(&dir), None);
        for f in ["libnvrtc-builtins.so.12.9", "libnvrtc.so.12.9.86", "libnvrtc.so"] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        assert_eq!(super::bundled_nvrtc(&dir), None, "only the soname form counts");
        std::fs::write(dir.join("libnvrtc.so.9"), b"").unwrap();
        std::fs::write(dir.join("libnvrtc.so.12.9.86"), b"x").unwrap();
        std::os::unix::fs::symlink("libnvrtc.so.12.9.86", dir.join("libnvrtc.so.12")).unwrap();
        std::os::unix::fs::symlink("missing", dir.join("libnvrtc.so.13")).unwrap();
        // 12 beats 9 as a number (not as text); the dangling .13 does not count
        assert_eq!(super::bundled_nvrtc(&dir), Some(dir.join("libnvrtc.so.12")));
        // the registry follows the real file and the driver
        let nvrtc = dir.join("libnvrtc.so.12");
        let a = super::nvrtc_registry_name(&nvrtc, Some("595.45.04"));
        assert!(a.starts_with("registry.libnvrtc.so.12.9.86.nv-595.45.04."), "{a}");
        assert_ne!(a, super::nvrtc_registry_name(&nvrtc, None));
        assert_ne!(a, super::nvrtc_registry_name(&nvrtc, Some("600.10")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

/// Windows: sweep updater leftovers (UPDATE_PATH.md §3.4) and, when running from the
/// portable bundle, point GStreamer/QML at the bundled payload (PACKAGING.md §1).
#[cfg(windows)]
pub(crate) fn early_startup() {
    // Shortcut launches (Start menu/desktop) begin with CWD inside current\ — and a
    // process's CWD is an open directory handle, which is exactly what blocks the
    // updater's current->backup rename (os error 32; found live on the 0.2.0->0.3.0
    // rollout: every "Update now" downloaded fine, failed the swap, re-armed the
    // banner). Hop to the install root before anything can depend on the CWD.
    if is_bundle_install() {
        let _ = std::env::set_current_dir(install_root());
    }
    redirect_stderr_to_log();
    install_crash_breadcrumb();
    cleanup_after_update();
    init_bundle_env();
    // Toast attribution (§8.2) — background thread: two reg.exe writes, and no
    // toast can fire before the sync loop is up anyway.
    std::thread::spawn(register_aumid);
}

/// Last-chance crash breadcrumb: a native (non-Rust) crash — Qt, GStreamer, a driver —
/// bypasses the Rust panic hook entirely and left NO trace in the log: the process just
/// vanished mid-call (a field report: ghost participant until SFU timeout, zero
/// evidence to triage). This SEH filter writes the exception code to the
/// (already-redirected) stderr handle using only WriteFile — no allocation, no CRT — and
/// then continues the search so WER/debuggers still see the crash.
/// 11.2.1: the code alone never said WHERE (seven monitor-drag crashes in one field report
/// were seven identical "0xC0000005" lines), so the filter also writes the
/// faulting module + offset, the access (read / write / execute of which address), the
/// thread, and the first frames of the stack as module+offset (unwound from the
/// exception's own context with the OS unwinder, bounded by the thread's stack limits).
/// Everything stays in stack buffers. TINHAT_CRASH_TEST=1 crashes on purpose right after
/// install (a null write) to check this output on a real Windows build.
#[cfg(windows)]
static CRASH_MAIN_THREAD: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[cfg(windows)]
mod crashlog {
    use std::ffi::c_void;
    extern "system" {
        pub fn GetStdHandle(n_std_handle: u32) -> isize;
        pub fn WriteFile(h: isize, buf: *const u8, len: u32, written: *mut u32, overlapped: *mut c_void) -> i32;
        pub fn GetModuleHandleExW(flags: u32, name: *const c_void, module: *mut isize) -> i32;
        pub fn GetModuleFileNameA(module: isize, buf: *mut u8, size: u32) -> u32;
        pub fn GetCurrentThreadId() -> u32;
    }
    #[cfg(target_arch = "x86_64")]
    extern "system" {
        pub fn GetCurrentThreadStackLimits(low: *mut usize, high: *mut usize);
        pub fn RtlLookupFunctionEntry(pc: u64, image_base: *mut u64, history: *mut c_void) -> *mut c_void;
        pub fn RtlVirtualUnwind(
            handler_type: u32,
            image_base: u64,
            pc: u64,
            entry: *mut c_void,
            context: *mut c_void,
            handler_data: *mut *mut c_void,
            establisher: *mut u64,
            pointers: *mut c_void,
        ) -> *mut c_void;
    }
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";

    /// One output line in a fixed stack buffer (anything past 400 bytes is dropped).
    pub struct Line {
        buf: [u8; 400],
        len: usize,
    }
    impl Line {
        pub fn new() -> Self {
            Line { buf: [0; 400], len: 0 }
        }
        pub fn s(&mut self, t: &[u8]) -> &mut Self {
            for &b in t {
                if self.len < self.buf.len() {
                    self.buf[self.len] = b;
                    self.len += 1;
                }
            }
            self
        }
        /// `0x` + hex, no leading zeros; `width` 8 pads to 8 digits (the exception code).
        pub fn hex(&mut self, v: u64, width: usize) -> &mut Self {
            self.s(b"0x");
            let mut started = false;
            for i in (0..16).rev() {
                let d = ((v >> (i * 4)) & 0xF) as usize;
                if d != 0 || started || i == 0 || i < width {
                    started = true;
                    self.s(&[DIGITS[d]]);
                }
            }
            self
        }
        pub fn dec(&mut self, mut v: u64) -> &mut Self {
            let mut tmp = [0u8; 20];
            let mut n = 0;
            loop {
                tmp[n] = b'0' + (v % 10) as u8;
                n += 1;
                v /= 10;
                if v == 0 {
                    break;
                }
            }
            while n > 0 {
                n -= 1;
                self.s(&[tmp[n]]);
            }
            self
        }
        /// `<module file name>+0x<offset>`, or the bare address when no module holds it.
        pub fn place(&mut self, addr: u64) -> &mut Self {
            const FROM_ADDRESS: u32 = 0x4;
            const UNCHANGED_REFCOUNT: u32 = 0x2;
            let mut module: isize = 0;
            let found = unsafe {
                GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, addr as *const c_void, &mut module) != 0
            };
            if !found || module == 0 {
                return self.s(b"?").s(b"+").hex(addr, 0);
            }
            let mut path = [0u8; 300];
            let n = unsafe { GetModuleFileNameA(module, path.as_mut_ptr(), path.len() as u32) } as usize;
            let n = n.min(path.len());
            let mut from = 0;
            for i in 0..n {
                if path[i] == b'\\' || path[i] == b'/' {
                    from = i + 1;
                }
            }
            if n > from {
                self.s(&path[from..n]);
            } else {
                self.s(b"?");
            }
            self.s(b"+").hex(addr.wrapping_sub(module as u64), 0)
        }
        pub fn write(&mut self) {
            self.s(b"\n");
            unsafe {
                let mut w: u32 = 0;
                WriteFile(GetStdHandle(STD_ERROR_HANDLE), self.buf.as_ptr(), self.len as u32, &mut w, std::ptr::null_mut());
            }
            self.len = 0;
        }
    }

    /// The first frames of the faulting thread's stack, unwound from a COPY of the exception's context.
    #[cfg(target_arch = "x86_64")]
    pub unsafe fn stack(ctx: *const u8) {
        // x64 CONTEXT: 0x4D0 bytes, 16-aligned; Rsp at 0x98, Rip at 0xF8.
        #[repr(C, align(16))]
        struct Ctx([u8; 0x4D0]);
        if ctx.is_null() {
            return;
        }
        let mut c = Ctx([0; 0x4D0]);
        std::ptr::copy_nonoverlapping(ctx, c.0.as_mut_ptr(), 0x4D0);
        let (mut low, mut high) = (0usize, 0usize);
        GetCurrentThreadStackLimits(&mut low, &mut high);
        let mut line = Line::new();
        line.s(b"[crash] stack:");
        for frame in 0..24u64 {
            let rip = std::ptr::read_unaligned(c.0.as_ptr().add(0xF8) as *const u64);
            let rsp = std::ptr::read_unaligned(c.0.as_ptr().add(0x98) as *const u64);
            if rip == 0 || (rsp as usize) < low || (rsp as usize) >= high {
                break;
            }
            if frame > 0 && frame % 4 == 0 {
                line.write();
                line.s(b"[crash] stack:");
            }
            line.s(b" #").dec(frame).s(b" ").place(rip);
            let mut base: u64 = 0;
            let entry = RtlLookupFunctionEntry(rip, &mut base, std::ptr::null_mut());
            if entry.is_null() {
                // a leaf function: the return address is at the top of the stack
                if (rsp as usize) + 8 > high {
                    break;
                }
                let ret = std::ptr::read_unaligned(rsp as *const u64);
                std::ptr::write_unaligned(c.0.as_mut_ptr().add(0xF8) as *mut u64, ret);
                std::ptr::write_unaligned(c.0.as_mut_ptr().add(0x98) as *mut u64, rsp + 8);
            } else {
                let mut data: *mut c_void = std::ptr::null_mut();
                let mut est: u64 = 0;
                RtlVirtualUnwind(0, base, rip, entry, c.0.as_mut_ptr() as *mut c_void, &mut data, &mut est, std::ptr::null_mut());
            }
        }
        line.write();
    }
    #[cfg(not(target_arch = "x86_64"))]
    pub unsafe fn stack(_ctx: *const u8) {}
}

#[cfg(windows)]
fn install_crash_breadcrumb() {
    use std::ffi::c_void;
    use std::sync::atomic::Ordering;
    extern "system" {
        fn SetUnhandledExceptionFilter(
            f: Option<unsafe extern "system" fn(*mut c_void) -> i32>,
        ) -> *mut c_void;
    }
    unsafe extern "system" fn filter(info: *mut c_void) -> i32 {
        use crashlog::Line;
        // EXCEPTION_POINTERS = { ExceptionRecord*, ContextRecord* }; x64 EXCEPTION_RECORD: code u32 @0,
        // ExceptionAddress @0x10, NumberParameters u32 @0x18, ExceptionInformation[15] u64 @0x20.
        let (rec, ctx) = if info.is_null() {
            (std::ptr::null(), std::ptr::null())
        } else {
            let p = info as *const *const u8;
            (*p, *p.add(1))
        };
        let code: u32 = if rec.is_null() { 0 } else { std::ptr::read_unaligned(rec as *const u32) };
        let mut line = Line::new();
        // the first line stays exactly what 0.8.x–11.2.0 wrote (log readers grep for it)
        line.s(b"\n[crash] unhandled native exception ").hex(code as u64, 8).write();
        if rec.is_null() {
            return 0;
        }
        let addr = std::ptr::read_unaligned(rec.add(0x10) as *const u64);
        let nparams = std::ptr::read_unaligned(rec.add(0x18) as *const u32);
        line.s(b"[crash] at ").place(addr);
        if code == 0xC000_0005 && nparams >= 2 {
            let kind = std::ptr::read_unaligned(rec.add(0x20) as *const u64);
            let target = std::ptr::read_unaligned(rec.add(0x28) as *const u64);
            line.s(match kind {
                0 => b" reading " as &[u8],
                1 => b" writing ",
                8 => b" executing ",
                _ => b" touching ",
            })
            .hex(target, 0);
        }
        let tid = crashlog::GetCurrentThreadId();
        let main = CRASH_MAIN_THREAD.load(Ordering::Relaxed);
        line.s(b" on thread ").dec(tid as u64).s(if tid == main { b" (the main thread)" as &[u8] } else { b" (not the main thread)" });
        line.write();
        // a stack overflow has no stack left to walk with
        if code != 0xC000_00FD {
            crashlog::stack(ctx);
        }
        0 // EXCEPTION_CONTINUE_SEARCH — WER/debugger takes over from here
    }
    CRASH_MAIN_THREAD.store(unsafe { crashlog::GetCurrentThreadId() }, Ordering::Relaxed);
    unsafe {
        SetUnhandledExceptionFilter(Some(filter));
    }
    if std::env::var_os("TINHAT_CRASH_TEST").is_some() {
        eprintln!("[crash] TINHAT_CRASH_TEST: crashing on purpose (null write)");
        unsafe { std::ptr::write_volatile(std::ptr::null_mut::<u8>(), 1) };
    }
}

/// Our AppUserModelID — toast attribution and the Start-menu / desktop shortcuts' identity
/// (windows/installer.iss sets the same on the shortcuts it makes). The same reverse-DNS id
/// as the Flatpak app id and the Inno AppId.
#[cfg(not(target_os = "linux"))]
pub(crate) const AUMID: &str = "dev.tinhat.TinHat";

/// Register the AUMID in HKCU (polish §8.2): the documented unpackaged-app route —
/// `HKCU\Software\Classes\AppUserModelId\<aumid>` with DisplayName (+IconUri when the
/// bundle ships tinhat.ico) — makes WinRT toasts show "Tin Hat" instead of
/// "Windows PowerShell", with no installer/shortcut needed. Idempotent (`reg add /f`),
/// best-effort, hidden windows (CREATE_NO_WINDOW: the exe is GUI-subsystem, a bare
/// spawned console would flash).
#[cfg(windows)]
fn register_aumid() {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let key = format!("HKCU\\Software\\Classes\\AppUserModelId\\{AUMID}");
    let mut values: Vec<(&str, String)> = vec![("DisplayName", "Tin Hat".to_string())];
    if let Some(icon) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("tinhat.ico")))
        .filter(|p| p.is_file())
    {
        values.push(("IconUri", icon.display().to_string()));
    }
    for (name, val) in values {
        let _ = std::process::Command::new("reg")
            .args(["add", &key, "/v", name, "/t", "REG_SZ", "/d", &val, "/f"])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
    }
    eprintln!("[platform] AUMID registered for toasts: {AUMID}");
}

/// GUI launches (shortcut/Explorer) have NO console, so every eprintln — the app's
/// entire diagnostic surface ([call]/[mic-rt]/[aec]/[platform]…) — went nowhere.
/// Point the process std handles at `<data>\logs\tinhat.log` instead, rotating
/// the previous launch's log to `.old.log`. A dev terminal keeps live output (the
/// redirect only fires when no console is attached). Linux twin: the cfg(unix)
/// dup2-to-logfile path. Rust's std stdio queries GetStdHandle per write, so a
/// SetStdHandle here catches every later eprintln; Qt/GStreamer DLLs cache their own
/// CRT streams (different CRT — the known boundary) and keep their output, which is
/// fine: GST_DEBUG has GST_DEBUG_FILE, and Qt warnings surface in dev runs.
#[cfg(windows)]
fn redirect_stderr_to_log() {
    extern "system" {
        fn GetStdHandle(n_std_handle: u32) -> isize;
        fn SetStdHandle(n_std_handle: u32, handle: isize) -> i32;
        fn AttachConsole(process_id: u32) -> i32;
    }
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    const INVALID_HANDLE: isize = -1;

    // The exe is /SUBSYSTEM:WINDOWS (no console of its own), so route in priority order:
    //  1. Parent gave us a redirected stderr (cmd `2> file`, Start-Process -Redirect…,
    //     a pipe) — keep it untouched; test harnesses depend on this.
    //  2. Parent HAS a console (dev terminal run) — attach and bind CONOUT$ so
    //     eprintln lands in that terminal, like a console app.
    //  3. Neither (Explorer/shortcut launch) — log file with one-deep rotation.
    let err = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    if err != 0 && err != INVALID_HANDLE {
        // Case 1: inherited redirect/pipe — leave it alone. When the parent is our own
        // self-updater (TINHAT_RELAUNCH), that inherited handle IS the previous session's
        // log file: print a separator so the merged file reads as two sessions instead
        // of one silently-glued blob.
        if std::env::var_os("TINHAT_RELAUNCH").is_some() {
            eprintln!("\n=== relaunched by the self-updater — same log file, previous session above ===");
        }
        return;
    }
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } != 0 {
        // case 2: bind the attached console's output as our std streams.
        if let Ok(f) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
            use std::os::windows::io::AsRawHandle;
            let h = f.as_raw_handle() as isize;
            unsafe {
                SetStdHandle(STD_ERROR_HANDLE, h);
                SetStdHandle(STD_OUTPUT_HANDLE, h);
            }
            std::mem::forget(f);
        }
        return;
    }
    // case 3: no console anywhere — the log file.
    let dir = app_data_dir().join("logs");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("tinhat.log");
    let old = dir.join("tinhat.old.log");
    let _ = std::fs::remove_file(&old);
    let _ = std::fs::rename(&path, &old);
    if let Ok(f) = std::fs::File::create(&path) {
        use std::os::windows::io::AsRawHandle;
        let h = f.as_raw_handle() as isize;
        unsafe {
            SetStdHandle(STD_ERROR_HANDLE, h);
            SetStdHandle(STD_OUTPUT_HANDLE, h);
        }
        // The handle now backs the process std streams for the app's lifetime.
        std::mem::forget(f);
        eprintln!("[platform] no console — logging to {}", path.display());
    }
}

// ---------------------------------------------------------------------------------
// Windows portable-bundle support (everything below is windows-only)
// ---------------------------------------------------------------------------------

/// Updater root = `%LOCALAPPDATA%\TinHatApp` (NOT derived from the exe path — a
/// pinned shortcut to a moved exe must still find `current\`/`backup\`; UPDATE_PATH §9).
#[cfg(windows)]
fn install_root() -> PathBuf {
    app_data_dir()
}

/// True when this process runs from the portable install's live bundle
/// (`<root>\current\tinhat.exe`) — the only layout the folder-swap updater serves.
#[cfg(windows)]
pub(crate) fn is_bundle_install() -> bool {
    match std::env::current_exe() {
        Ok(exe) => exe
            .parent()
            .map(|d| d.eq_ignore_ascii_case_path(&install_root().join("current")))
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// Case-insensitive path compare (Windows filesystems are case-preserving/insensitive).
#[cfg(windows)]
trait EqIgnoreCasePath {
    fn eq_ignore_ascii_case_path(&self, other: &Path) -> bool;
}
#[cfg(windows)]
impl EqIgnoreCasePath for Path {
    fn eq_ignore_ascii_case_path(&self, other: &Path) -> bool {
        self.to_string_lossy().to_lowercase() == other.to_string_lossy().to_lowercase()
    }
}

/// Bundle runtime env (PACKAGING.md §1): when the exe dir carries the bundled GStreamer
/// tree, force GStreamer to scan ONLY our plugins (a machine-wide GStreamer install must
/// not leak in), point the out-of-process scanner at the bundled one, and keep the
/// registry cache in the writable data root. Same idea for the bundled `qml\` tree.
/// Also: default the QQC2 style to `Basic` — the native "windows" style ignores
/// `Kirigami.Theme`/palette overrides and spams "customization of this control is not
/// supported" (TEST_PLAN.md §2.5); Basic is the palette-driven no-dependency style.
/// Every set here respects a pre-existing env override.
#[cfg(windows)]
fn init_bundle_env() {
    // NOTE: the authoritative style force is QQuickStyle::setStyle in the C++ shim
    // (video.rs::force_quick_style) — an env var set HERE is not reliably visible to
    // Qt's DLLs across the static-CRT boundary (verified live). This set only helps
    // child processes we spawn inherit the intent.
    if std::env::var_os("QT_QUICK_CONTROLS_STYLE").is_none() {
        std::env::set_var("QT_QUICK_CONTROLS_STYLE", "Basic");
    }
    // The streamer crate persists its small settings (mic/output/camera picks, the
    // AEC/AGC/denoise toggles, share quality…) under `$XDG_CONFIG_HOME || $HOME/.config`.
    // Neither is set on Windows, so those paths degraded to a CWD-RELATIVE `.config\` —
    // for a bundle install that is `current\.config`, which the folder-swap updater
    // DELETES on every update. XDG_CONFIG_HOME points into the stable data root (user
    // data lives at the root, surviving swaps — UPDATE_PATH.md §2). Rust code reads its
    // own process env reliably (the CRT-boundary caveat above applies only to Qt DLLs).
    // Same class of bug for CACHES: room.rs's timeline/members/media caches read
    // XDG_CACHE_HOME directly (not platform::cache_base_dir), so they degraded to a
    // CWD-relative `.cache\` too — which in a dev checkout is INSIDE THE REPO (room
    // cache JSONs nearly got committed) and in a bundle install is inside current\.
    // XDG_CACHE_HOME gets cache_base_dir()'s own Windows answer so both routes agree.
    // Both are exported by init_dirs() at the top of main() now — here they were
    // set only after the safe-mode decision had used a relative `.config`. The call is a
    // no-op when main() already made it; the config folder is still created here, and the
    // log lines land in the log file now.
    init_dirs();
    for (name, value) in EARLY_EXPORTS.get().map(|v| v.as_slice()).unwrap_or(&[]) {
        if *name == "XDG_CONFIG_HOME" {
            let _ = std::fs::create_dir_all(value);
        }
        eprintln!("[platform] {name} -> {}", value.display());
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(dir) = exe.parent() else { return };

    let gst_plugins = dir.join("lib").join("gstreamer-1.0");
    if gst_plugins.is_dir() {
        if std::env::var_os("GST_PLUGIN_SYSTEM_PATH").is_none() {
            std::env::set_var("GST_PLUGIN_SYSTEM_PATH", &gst_plugins);
        }
        let scanner = dir.join("libexec").join("gstreamer-1.0").join("gst-plugin-scanner.exe");
        if scanner.is_file() && std::env::var_os("GST_PLUGIN_SCANNER").is_none() {
            std::env::set_var("GST_PLUGIN_SCANNER", &scanner);
        }
        if std::env::var_os("GST_REGISTRY").is_none() {
            let _ = std::fs::create_dir_all(app_data_dir());
            std::env::set_var("GST_REGISTRY", app_data_dir().join("gstregistry.bin"));
        }
        // gio TLS/proxy modules (glib-networking): souphttpsrc dies with
        // "streaming stopped, reason error (-5)" on EVERY https URL without a
        // TLS backend (soundboard's exact live failure). glib's implicit
        // <prefix>\lib\gio\modules probing depends on DLL-relative heuristics —
        // pin it explicitly.
        let gio_modules = dir.join("lib").join("gio").join("modules");
        if gio_modules.is_dir() && std::env::var_os("GIO_MODULE_DIR").is_none() {
            std::env::set_var("GIO_MODULE_DIR", &gio_modules);
            eprintln!("[platform] gio modules (TLS): {}", gio_modules.display());
        }
        eprintln!("[platform] bundle GStreamer: {}", gst_plugins.display());
    }

    let qml = dir.join("qml");
    if qml.is_dir() {
        // Append, never clobber — a dev override must win.
        let mut paths: Vec<PathBuf> = Vec::new();
        if let Some(existing) = std::env::var_os("QML_IMPORT_PATH") {
            paths.extend(std::env::split_paths(&existing));
        }
        paths.push(qml.clone());
        if let Ok(joined) = std::env::join_paths(paths) {
            std::env::set_var("QML_IMPORT_PATH", joined);
        }
        eprintln!("[platform] bundle QML imports: {}", qml.display());
    }

    // Watch-party: libmpv's ytdl_hook spawns `yt-dlp` found via PATH (the QML
    // deliberately sets no hardcoded path — see WatchPartyWindow.qml). The bundle
    // ships yt-dlp.exe next to the exe; the exe dir is NOT on PATH for a
    // double-clicked app, so prepend it. mpv runs in-process, so its subprocess
    // inherits this env. No-op when yt-dlp.exe isn't bundled.
    if dir.join("yt-dlp.exe").is_file() {
        let mut paths: Vec<PathBuf> = vec![dir.to_path_buf()];
        if let Some(existing) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&existing));
        }
        if let Ok(joined) = std::env::join_paths(paths) {
            std::env::set_var("PATH", joined);
        }
        eprintln!("[platform] yt-dlp.exe on PATH (watch-party downloader)");
    }
}

/// Next-launch sweep (UPDATE_PATH.md §3.4): the previous bundle (`backup\`) and any
/// aborted download (`staging\`) can only be deleted once the old process is gone —
/// which is now. Best-effort: a still-locked file (AV scan…) just stays for the launch
/// after; never fatal, never user-visible beyond one log line.
#[cfg(windows)]
pub(crate) fn cleanup_after_update() {
    let root = install_root();
    for sub in ["backup", "staging"] {
        let dir = root.join(sub);
        if dir.exists() {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => eprintln!("[platform] swept updater dir {}", dir.display()),
                Err(e) => eprintln!("[platform] sweep of {} deferred ({e}); retrying next launch", dir.display()),
            }
        }
    }
}

/// The Windows install step (UPDATE_PATH.md §3.2): verified zip bytes → staged unzip →
/// VERSION sanity → backup/promote folder swap with retries + rollback. Returns the
/// path of the promoted exe. The caller (update.rs) has ALREADY verified the sha256.
#[cfg(windows)]
pub(crate) fn install_update_zip(zip_bytes: &[u8], expected_version: &str) -> Result<PathBuf, String> {
    let root = install_root();
    let current = root.join("current");
    let staging = root.join("staging");
    let backup = root.join("backup");

    if !current.join("tinhat.exe").is_file() {
        return Err(format!(
            "no portable install at {} — self-update needs the bundle layout (first install = unzip there)",
            current.display()
        ));
    }

    // 1-2. Clean staging, unzip the new bundle into staging\current\.
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|e| format!("clear staging: {e}"))?;
    }
    let stage_bundle = staging.join("current");
    std::fs::create_dir_all(&stage_bundle).map_err(|e| format!("mkdir staging: {e}"))?;
    let cursor = std::io::Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| format!("open zip: {e}"))?;
    archive.extract(&stage_bundle).map_err(|e| format!("unzip: {e}"))?;

    // Tolerate a zip whose top level is a single wrapper dir (PACKAGING §7 zips the
    // folder; UPDATE_PATH §1 zips the contents — accept both shapes).
    let mut bundle = stage_bundle.clone();
    if !bundle.join("tinhat.exe").is_file() {
        let entries: Vec<_> = std::fs::read_dir(&bundle)
            .map_err(|e| format!("read staging: {e}"))?
            .filter_map(|e| e.ok())
            .collect();
        if entries.len() == 1 && entries[0].path().is_dir()
            && entries[0].path().join("tinhat.exe").is_file()
        {
            bundle = entries[0].path();
        } else {
            return Err("zip has no tinhat.exe at top level or in a single wrapper dir".into());
        }
    }

    // Sanity: the bundle must declare the version the manifest promised (UPDATE_PATH §3.2/4).
    let ver_file = bundle.join("VERSION");
    let staged_ver = std::fs::read_to_string(&ver_file).unwrap_or_default().trim().to_string();
    if !expected_version.is_empty() && staged_ver != expected_version {
        return Err(format!(
            "staged VERSION {staged_ver:?} != manifest version {expected_version:?} (bad bundle?)"
        ));
    }

    // 3. Sweep any stale backup (best-effort — a locked leftover must not block updating).
    if backup.exists() {
        if let Err(e) = std::fs::remove_dir_all(&backup) {
            eprintln!("[platform] stale backup sweep failed ({e}); proceeding");
        }
    }

    // 4-5. The swap: current -> backup, staged bundle -> current. Same-volume renames
    // are legal on a live bundle (the running exe keeps its handles). Retry each rename
    // ~5x/200ms for transient AV locks (UPDATE_PATH §3.5); roll back if promote fails.
    rename_retry(&current, &backup).map_err(|e| format!("move live bundle aside: {e}"))?;
    if let Err(e) = rename_retry(&bundle, &current) {
        let msg = match rename_retry(&backup, &current) {
            Ok(()) => format!("promote new bundle: {e} (rolled back to previous version)"),
            Err(rb) => format!("promote new bundle: {e}; ROLLBACK ALSO FAILED: {rb} — reinstall from zip"),
        };
        return Err(msg);
    }
    let _ = std::fs::remove_dir_all(&staging); // leftover wrapper dirs; best-effort

    Ok(current.join("tinhat.exe"))
}

// ---------------------------------------------------------------------------------
// Windows: "Install to your apps" from a portable (zip) run + uninstall (0.8.16)
//
// Field reality (2026-09-01): users download the zip, extract it into
// Downloads, double-click the exe, get the SmartScreen prompt, and then — because the
// folder-swap updater only serves `<root>\current` — every update told them to
// "unzip a release into <data root>\current", a folder that does not
// exist on a fresh machine, so they re-downloaded the zip for every release. The
// Inno Setup installer (windows/installer.iss) is the first-run vehicle now; this
// block makes a zip run install ITSELF the same way (copy → swap → shortcuts → Apps &
// features), after which the in-app updater owns updates like any other install.
// ---------------------------------------------------------------------------------

/// True when this process runs a RELEASE bundle from somewhere other than the install
/// root's `current\` — typically a zip extracted into Downloads. A release bundle is
/// recognised by the publisher's VERSION file next to the exe; a dev build under
/// target\release has none and is NOT a portable run.
#[cfg(windows)]
pub(crate) fn is_portable_run() -> bool {
    if is_bundle_install() {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    let Some(dir) = exe.parent() else { return false };
    dir.join("VERSION").is_file() && dir.join("tinhat.exe").is_file()
}

/// VERSION of the bundle installed at `<root>\current`, if one is installed.
#[cfg(windows)]
pub(crate) fn installed_bundle_version() -> Option<String> {
    let cur = install_root().join("current");
    if !cur.join("tinhat.exe").is_file() {
        return None;
    }
    let v = std::fs::read_to_string(cur.join("VERSION"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    Some(if v.is_empty() { "?".to_string() } else { v })
}

#[cfg(windows)]
fn install_prompt_marker() -> PathBuf {
    install_root().join("install-prompt-seen")
}
/// Has the first-run "install to your apps?" prompt been answered (either way)?
#[cfg(windows)]
pub(crate) fn install_prompt_seen() -> bool {
    install_prompt_marker().is_file()
}
#[cfg(windows)]
pub(crate) fn mark_install_prompt_seen() {
    let _ = std::fs::create_dir_all(install_root());
    let _ = std::fs::write(install_prompt_marker(), "1");
}

/// Copy THIS bundle (the exe's directory) into the self-updater layout and register
/// it as an app: staging copy → swap into `<root>\current` (a previous install moves
/// to backup\, exactly like an update), strip Mark-of-the-Web, Start-menu + desktop
/// shortcuts, an Apps & features entry. Returns the installed exe. Runs OFF the GUI
/// thread (hundreds of MB to copy). Same-volume rename rules as install_update_zip.
#[cfg(windows)]
pub(crate) fn install_from_portable() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let src = exe.parent().ok_or("exe has no parent dir")?.to_path_buf();
    let root = install_root();
    let current = root.join("current");
    let staging = root.join("staging");
    let backup = root.join("backup");
    if src.eq_ignore_ascii_case_path(&current) {
        return Ok(current.join("tinhat.exe"));
    }
    std::fs::create_dir_all(&root).map_err(|e| format!("mkdir {}: {e}", root.display()))?;
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|e| format!("clear staging: {e}"))?;
    }
    let stage_bundle = staging.join("current");
    copy_dir_recursive(&src, &stage_bundle)?;
    if !stage_bundle.join("tinhat.exe").is_file() {
        return Err("copy produced no tinhat.exe".into());
    }
    strip_mark_of_the_web(&stage_bundle);
    if backup.exists() {
        if let Err(e) = std::fs::remove_dir_all(&backup) {
            eprintln!("[install] stale backup sweep failed ({e}); proceeding");
        }
    }
    if current.exists() {
        rename_retry(&current, &backup).map_err(|e| format!("move the previous install aside: {e}"))?;
    }
    if let Err(e) = rename_retry(&stage_bundle, &current) {
        if backup.exists() {
            let _ = rename_retry(&backup, &current);
        }
        return Err(format!("promote: {e}"));
    }
    let _ = std::fs::remove_dir_all(&staging);
    let installed = current.join("tinhat.exe");
    let version = std::fs::read_to_string(current.join("VERSION"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    create_shortcuts(&installed);
    register_uninstall_entry(&installed, &version);
    mark_install_prompt_seen();
    eprintln!("[install] {} installed -> {}", version, installed.display());
    Ok(installed)
}

#[cfg(windows)]
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("mkdir {}: {e}", dst.display()))?;
    for entry in std::fs::read_dir(src).map_err(|e| format!("read {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let p = entry.path();
        let target = dst.join(entry.file_name());
        let ft = entry.file_type().map_err(|e| e.to_string())?;
        if ft.is_dir() {
            copy_dir_recursive(&p, &target)?;
        } else if ft.is_file() {
            std::fs::copy(&p, &target).map_err(|e| format!("copy {}: {e}", p.display()))?;
        }
    }
    Ok(())
}

/// Delete the `Zone.Identifier` alternate data stream Explorer's zip extraction stamps
/// on every file — that stream (Mark-of-the-Web) is what makes SmartScreen interrogate
/// an unsigned exe on launch; CopyFile carries it along. DeleteFileW on
/// `<file>:Zone.Identifier` removes just the stream. Best-effort, per file.
#[cfg(windows)]
fn strip_mark_of_the_web(dir: &Path) {
    extern "system" {
        fn DeleteFileW(lp_file_name: *const u16) -> i32;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            strip_mark_of_the_web(&p);
            continue;
        }
        use std::os::windows::ffi::OsStrExt;
        let mut w: Vec<u16> = p.as_os_str().encode_wide().collect();
        w.extend(":Zone.Identifier".encode_utf16());
        w.push(0);
        unsafe {
            DeleteFileW(w.as_ptr());
        }
    }
}

#[cfg(windows)]
fn run_hidden(program: &str, args: &[&str]) -> Option<std::process::Output> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()
}

/// Start-menu + desktop shortcuts via the shell's COM object (no extra crate). Start-in
/// is the install ROOT, never current\ — a CWD inside current\ self-locks the updater's
/// folder swap (the 0.2.0→0.3.0 lesson; the installer's [Icons] says the same).
#[cfg(windows)]
fn create_shortcuts(exe: &Path) {
    let root = install_root();
    let icon = exe
        .parent()
        .map(|d| d.join("tinhat.ico"))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| exe.to_path_buf());
    let ps = format!(
        "$s = New-Object -ComObject WScript.Shell; \
         foreach ($dir in @([Environment]::GetFolderPath('Programs'), [Environment]::GetFolderPath('Desktop'))) {{ \
           if (-not $dir) {{ continue }}; \
           $l = $s.CreateShortcut((Join-Path $dir 'Tin Hat.lnk')); \
           $l.TargetPath = '{exe}'; $l.WorkingDirectory = '{root}'; $l.IconLocation = '{icon},0'; \
           $l.Description = 'Tin Hat'; $l.Save() }}",
        exe = exe.display(),
        root = root.display(),
        icon = icon.display()
    );
    match run_hidden(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &ps],
    ) {
        Some(o) if o.status.success() => eprintln!("[install] Start-menu + desktop shortcuts created"),
        Some(o) => eprintln!(
            "[install] shortcut creation failed: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        None => eprintln!("[install] shortcut creation failed: powershell not runnable"),
    }
}

#[cfg(windows)]
const PORTABLE_UNINSTALL_KEY: &str =
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\TinHatApp";
/// The Inno Setup installer's own entry (AppId dev.tinhat.TinHat) — when it
/// exists, the installer owns Apps & features; we must not add a duplicate.
#[cfg(windows)]
const INNO_UNINSTALL_KEY: &str =
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\dev.tinhat.TinHat_is1";

#[cfg(windows)]
fn reg_key_exists(key: &str) -> bool {
    run_hidden("reg", &["query", key]).map(|o| o.status.success()).unwrap_or(false)
}

#[cfg(windows)]
fn register_uninstall_entry(exe: &Path, version: &str) {
    if reg_key_exists(INNO_UNINSTALL_KEY) {
        eprintln!("[install] installer-owned Apps & features entry present; not adding a second one");
        return;
    }
    let root = install_root();
    let uninstall = format!("\"{}\" --uninstall", exe.display());
    let icon = exe.display().to_string();
    let vals: [(&str, &str, String); 8] = [
        ("DisplayName", "REG_SZ", "Tin Hat".into()),
        ("DisplayVersion", "REG_SZ", version.into()),
        ("Publisher", "REG_SZ", "Tin Hat".into()),
        ("InstallLocation", "REG_SZ", root.display().to_string()),
        ("DisplayIcon", "REG_SZ", icon),
        ("UninstallString", "REG_SZ", uninstall),
        ("NoModify", "REG_DWORD", "1".into()),
        ("NoRepair", "REG_DWORD", "1".into()),
    ];
    for (name, ty, val) in vals.iter() {
        let (name, ty, val): (&str, &str, &str) = (name, ty, val.as_str());
        let _ = run_hidden(
            "reg",
            &["add", PORTABLE_UNINSTALL_KEY, "/v", name, "/t", ty, "/d", val, "/f"],
        );
    }
    eprintln!("[install] Apps & features entry registered");
}

/// `tinhat.exe --uninstall`: confirm, drop shortcuts + registry entries, and
/// delete the bundle folders once this process has exited (they are in use while it
/// lives). Sign-in, keys and settings under the root are deliberately kept — a
/// reinstall picks up where the user left off (same as most Windows apps).
#[cfg(windows)]
pub(crate) fn uninstall_portable_install() -> i32 {
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, utype: u32) -> i32;
    }
    const MB_YESNO: u32 = 0x0000_0004;
    const MB_ICONQUESTION: u32 = 0x0000_0020;
    const MB_DEFBUTTON2: u32 = 0x0000_0100;
    const IDYES: i32 = 6;
    let to_w = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let text = to_w(
        "Remove Tin Hat from this PC?\n\nYour sign-in and settings stay in place, so \
         installing it again later picks up where you left off.",
    );
    let caption = to_w("Uninstall Tin Hat");
    let answer = unsafe {
        MessageBoxW(0, text.as_ptr(), caption.as_ptr(), MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2)
    };
    if answer != IDYES {
        return 0;
    }
    let ps = "foreach ($dir in @([Environment]::GetFolderPath('Programs'), [Environment]::GetFolderPath('Desktop'))) { \
                if ($dir) { Remove-Item -LiteralPath (Join-Path $dir 'Tin Hat.lnk') -Force -ErrorAction SilentlyContinue } }";
    let _ = run_hidden(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", ps],
    );
    let _ = run_hidden("reg", &["delete", PORTABLE_UNINSTALL_KEY, "/f"]);
    let aumid_key = format!("HKCU\\Software\\Classes\\AppUserModelId\\{AUMID}");
    let _ = run_hidden("reg", &["delete", &aumid_key, "/f"]);
    // The bundle folders hold THIS exe: delete them after we are gone (ping = sleep 3s
    // without a console dependency), detached and windowless.
    let root = install_root();
    let script = format!(
        "ping -n 4 127.0.0.1 >nul & rmdir /s /q \"{c}\" & rmdir /s /q \"{b}\" & rmdir /s /q \"{s}\" & del /q \"{m}\"",
        c = root.join("current").display(),
        b = root.join("backup").display(),
        s = root.join("staging").display(),
        m = install_prompt_marker().display()
    );
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        let _ = std::process::Command::new("cmd")
            .args(["/c", &script])
            .current_dir(&root)
            .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
            .spawn();
    }
    0
}

#[cfg(windows)]
fn rename_retry(from: &Path, to: &Path) -> Result<(), String> {
    let mut last = String::new();
    for attempt in 0..5 {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = e.to_string();
                if attempt < 4 {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
            }
        }
    }
    Err(format!("{} -> {}: {last}", from.display(), to.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M10: the private-folder and private-file helpers (Unix modes; a temp folder of the test's own).
    #[cfg(unix)]
    #[test]
    fn private_folders_and_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let base = std::env::temp_dir().join(format!("tinhat-m10-{}-{}", std::process::id(), line!()));
        let _ = std::fs::remove_dir_all(&base);
        // a fresh install: created 0700 (and its missing parent too)
        let fresh = base.join("share").join("TinHat");
        assert!(!make_private_dir(&fresh).unwrap());
        assert_eq!(mode(&fresh) & 0o077, 0);
        assert_eq!(mode(&base.join("share")) & 0o077, 0);
        // an existing install at the old default: narrowed once, then left alone
        let old = base.join("cache-TinHat");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(make_private_dir(&old).unwrap());
        assert_eq!(mode(&old), 0o700);
        assert!(!make_private_dir(&old).unwrap());
        // session.json: a new file is 0600 from its creation; an old 0644 one is narrowed before the token goes in
        let f = fresh.join("session.json");
        write_private_file(&f, b"{\"a\":1}").unwrap();
        assert_eq!(mode(&f), 0o600);
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private_file(&f, b"{\"b\":2}").unwrap();
        assert_eq!(mode(&f), 0o600);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "{\"b\":2}");
        let _ = std::fs::remove_dir_all(&base);
    }
    use std::collections::HashMap;
    use std::ffi::OsString;

    /// An environment as data: `(name, value)` pairs; anything else is unset.
    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let m: HashMap<String, OsString> = pairs.iter().map(|(k, v)| (k.to_string(), OsString::from(v))).collect();
        move |k: &str| m.get(k).cloned()
    }

    /// `env` plus the variables a plan exports (what main()'s first line leaves behind).
    fn with_exports(pairs: &[(&str, &str)], exports: &[(&'static str, PathBuf)]) -> impl Fn(&str) -> Option<OsString> {
        let mut m: HashMap<String, OsString> = pairs.iter().map(|(k, v)| (k.to_string(), OsString::from(v))).collect();
        for (k, v) in exports {
            m.insert(k.to_string(), v.clone().into_os_string());
        }
        move |k: &str| m.get(k).cloned()
    }

    /// Absolute by Windows rules (the production rule; Path::is_absolute answers for Linux).
    fn windows_absolute(p: &Path) -> bool {
        windows_is_absolute(p.as_os_str())
    }

    const LOCAL: &str = r"C:\Users\alice\AppData\Local";
    const TEMP: &str = r"C:\Users\alice\AppData\Local\Temp";

    /// A Windows start as the OS hands it over: no HOME, no XDG_*.
    const WIN: &[(&str, &str)] = &[("LOCALAPPDATA", LOCAL), ("USERPROFILE", r"C:\Users\alice"), ("TEMP", TEMP)];

    #[test]
    fn windows_exports_config_and_cache_into_the_data_root() {
        let plan = resolve_dirs(DirRules::Windows, &env(WIN), Path::new(TEMP));
        let config = Path::new(LOCAL).join("TinHatApp").join("config");
        assert_eq!(plan.config_home, config);
        assert_eq!(plan.cache_home, PathBuf::from(LOCAL));
        assert_eq!(
            plan.exports,
            vec![("XDG_CONFIG_HOME", config), ("XDG_CACHE_HOME", PathBuf::from(LOCAL))]
        );
        assert!(windows_absolute(&plan.config_home) && windows_absolute(&plan.cache_home));
    }

    /// The alternating safe-mode bug itself: the marker the safe-mode decision reads and arms (before
    /// init_bundle_env) and the one mark_boot_ok() clears (after it) are the same file,
    /// whatever folder Tin Hat was started from.
    #[test]
    fn windows_start_marker_is_read_and_cleared_in_one_file() {
        let temp = Path::new(TEMP);
        let first_line = resolve_dirs(DirRules::Windows, &env(WIN), temp);
        let after_line_one = with_exports(WIN, &first_line.exports);
        // safe_decision() (before init_bundle_env) and mark_boot_ok() (after it) both see
        // the environment main()'s first line left; init_bundle_env() exports nothing new.
        let at_decision = resolve_dirs(DirRules::Windows, &after_line_one, temp);
        assert!(at_decision.exports.is_empty(), "a second init_dirs() must be a no-op");
        let at_ok = resolve_dirs(DirRules::Windows, &after_line_one, temp);
        assert_eq!(at_decision.config_home, at_ok.config_home);
        assert_eq!(at_decision.config_home, first_line.config_home);
        assert!(windows_absolute(&at_decision.config_home), "never relative to the launch folder");
        // Before the fix the decision ran on the raw environment with the HOME fallback
        // (HOME is unset on Windows): a path relative to the launch folder — the bug.
        let pre_fix = resolve_dirs(DirRules::Unix, &env(WIN), temp);
        assert_eq!(pre_fix.config_home, PathBuf::from(".config"));
        assert_ne!(pre_fix.config_home, at_ok.config_home);
    }

    #[test]
    fn windows_keeps_a_directory_the_user_set() {
        let pairs = [WIN, &[("XDG_CONFIG_HOME", r"D:\cfg"), ("XDG_CACHE_HOME", r"D:\cache")][..]].concat();
        let plan = resolve_dirs(DirRules::Windows, &env(&pairs), Path::new(TEMP));
        assert_eq!(plan.config_home, PathBuf::from(r"D:\cfg"));
        assert_eq!(plan.cache_home, PathBuf::from(r"D:\cache"));
        assert!(plan.exports.is_empty());
    }

    #[test]
    fn windows_treats_an_empty_variable_as_unset() {
        // Not every reader filters "" (e2ee_keys.rs does not); the exported fallback replaces it.
        let pairs = [WIN, &[("XDG_CONFIG_HOME", ""), ("XDG_CACHE_HOME", "")][..]].concat();
        let plan = resolve_dirs(DirRules::Windows, &env(&pairs), Path::new(TEMP));
        assert_eq!(plan.config_home, Path::new(LOCAL).join("TinHatApp").join("config"));
        assert_eq!(plan.exports.len(), 2);
    }

    /// A relative value moves with the working folder — the same split again (armed under the
    /// launch folder, cleared under the install root after early_startup's chdir). Ignored and
    /// replaced by the exported absolute fallback, as the XDG spec asks.
    #[test]
    fn windows_ignores_relative_directory_variables() {
        for rel in ["cfg", r".\cfg", "../cfg", r"\cfg", "/cfg", "C:cfg", "C:"] {
            let pairs = [WIN, &[("XDG_CONFIG_HOME", rel), ("XDG_CACHE_HOME", rel)][..]].concat();
            let plan = resolve_dirs(DirRules::Windows, &env(&pairs), Path::new(TEMP));
            let config = Path::new(LOCAL).join("TinHatApp").join("config");
            assert_eq!(plan.config_home, config, "{rel:?}");
            assert_eq!(plan.cache_home, PathBuf::from(LOCAL), "{rel:?}");
            assert_eq!(plan.exports, vec![("XDG_CONFIG_HOME", config), ("XDG_CACHE_HOME", PathBuf::from(LOCAL))], "{rel:?}");
        }
        // Unix keeps the historical rule byte for byte (a relative value is used as it is).
        let unix = resolve_dirs(DirRules::Unix, &env(&[("HOME", "/home/a"), ("XDG_CONFIG_HOME", "cfg")]), Path::new("/tmp"));
        assert_eq!(unix.config_home, PathBuf::from("cfg"));
    }

    #[test]
    fn windows_absolute_spellings_are_kept() {
        for abs in [
            r"D:\cfg", "D:/cfg", r"d:\", r"\\server\share\cfg", "//server/share/cfg", r"\\?\D:\cfg", r"\\.\D:\cfg",
            r"\\?\UNC\server\share\cfg", r"\??\D:\cfg", r"\??\UNC\server\share\cfg",
        ] {
            assert!(windows_is_absolute(std::ffi::OsStr::new(abs)), "{abs:?}");
            let pairs = [WIN, &[("XDG_CONFIG_HOME", abs)][..]].concat();
            let plan = resolve_dirs(DirRules::Windows, &env(&pairs), Path::new(TEMP));
            assert_eq!(plan.config_home, PathBuf::from(abs), "{abs:?}");
            assert_eq!(plan.exports, vec![("XDG_CACHE_HOME", PathBuf::from(LOCAL))], "{abs:?}");
        }
        for rel in ["", "cfg", r"\cfg", "C:cfg", "C:", "1:\\x", r"\", "/??/D:/cfg", r"\?"] {
            assert!(!windows_is_absolute(std::ffi::OsStr::new(rel)), "{rel:?}");
        }
    }

    #[test]
    fn windows_without_localappdata_uses_temp_like_the_data_root() {
        for pairs in [
            &[("USERPROFILE", r"C:\Users\alice")][..],
            &[("LOCALAPPDATA", ""), ("USERPROFILE", r"C:\Users\alice")][..],
            &[("LOCALAPPDATA", r"AppData\Local"), ("USERPROFILE", r"C:\Users\alice")][..],
        ] {
            let plan = resolve_dirs(DirRules::Windows, &env(pairs), Path::new(TEMP));
            assert_eq!(windows_data_base(&env(pairs), Path::new(TEMP)), PathBuf::from(TEMP));
            assert_eq!(plan.config_home, Path::new(TEMP).join("TinHatApp").join("config"));
            assert_eq!(plan.cache_home, PathBuf::from(TEMP));
            assert!(windows_absolute(&plan.config_home));
        }
    }

    #[test]
    fn windows_data_base_is_localappdata() {
        assert_eq!(windows_data_base(&env(WIN), Path::new(TEMP)), PathBuf::from(LOCAL));
    }

    /// The data root (sign-in, keys, the installed app) must never be the cache folder the
    /// Rust caches use (`%LOCALAPPDATA%\TinHat\...`) or Qt's folders (`TinHat\tinhat`): the
    /// installer's Repair and Uninstall clear those wholesale. windows/installer.iss and
    /// windows/tinhat-maint.ps1 name the same root.
    #[test]
    fn windows_root_is_not_a_cache_folder() {
        assert_eq!(WINDOWS_ROOT_NAME, "TinHatApp");
        assert!(!WINDOWS_ROOT_NAME.eq_ignore_ascii_case("TinHat"));
        let plan = resolve_dirs(DirRules::Windows, &env(WIN), Path::new(TEMP));
        assert!(!plan.config_home.starts_with(Path::new(LOCAL).join("TinHat")));
    }

    /// Linux / Flatpak: the historical `$XDG_* || $HOME/...` answers, nothing exported.
    #[test]
    fn unix_keeps_the_historical_fallbacks_and_exports_nothing() {
        let t = Path::new("/tmp");
        let home = resolve_dirs(DirRules::Unix, &env(&[("HOME", "/home/a")]), t);
        assert_eq!(home.config_home, PathBuf::from("/home/a/.config"));
        assert_eq!(home.cache_home, PathBuf::from("/home/a/.cache"));
        assert!(home.exports.is_empty());
        let flatpak = resolve_dirs(
            DirRules::Unix,
            &env(&[
                ("HOME", "/home/a"),
                ("XDG_CONFIG_HOME", "/home/a/.var/app/dev.tinhat.TinHat/config"),
                ("XDG_CACHE_HOME", "/home/a/.var/app/dev.tinhat.TinHat/cache"),
            ]),
            t,
        );
        assert_eq!(flatpak.config_home, PathBuf::from("/home/a/.var/app/dev.tinhat.TinHat/config"));
        assert_eq!(flatpak.cache_home, PathBuf::from("/home/a/.var/app/dev.tinhat.TinHat/cache"));
        assert!(flatpak.exports.is_empty());
        let empty = resolve_dirs(DirRules::Unix, &env(&[("HOME", "/home/a"), ("XDG_CONFIG_HOME", "")]), t);
        assert_eq!(empty.config_home, PathBuf::from("/home/a/.config"));
        // No HOME at all: the historical relative answer, unchanged on purpose.
        assert_eq!(resolve_dirs(DirRules::Unix, &env(&[]), t).config_home, PathBuf::from(".config"));
    }

    /// The live resolver on this (Linux) test host follows the Unix rule.
    #[test]
    fn host_rules_match_the_build_target() {
        assert_eq!(DirRules::HOST, if cfg!(windows) { DirRules::Windows } else { DirRules::Unix });
    }
}
