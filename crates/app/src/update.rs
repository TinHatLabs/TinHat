//! UpdateController — in-app / on-startup update check + safe self-replace.
//!
//! The native client isn't in an app store, so it updates itself: on startup (and on
//! demand) it fetches a small JSON manifest, semver-compares it to the running version,
//! and surfaces an "update available" banner. "Update now" downloads the new binary,
//! verifies its sha256, atomically replaces `current_exe()`, and offers a restart.
//!
//! Feed (selfhost, 2026-10-21 — updates come from GitHub, not from a self-run update host): the manifests published
//! with every release of `TinHatLabs/TinHat` (the release tool (publish-desktop-github) writes them):
//! `https://github.com/TinHatLabs/TinHat/releases/latest/download/latest.json` (Linux: url = url_bin = the bare
//! `tinhat-<ver>-x86_64.bin`) and `…/latest-windows.json` (Windows: url = the `tinhat-<ver>-win64.zip` bundle,
//! `installer` = the Setup .exe, unused here); every URL pinned to that release's assets. `releases/latest` never
//! names a draft or a pre-release. GitHub answers with a redirect to its download host (followed); the requests carry
//! the app's User-Agent. Integrity as before: the sha256 in the manifest, fetched over HTTPS.
//!
//! Manifest (HTTPS, override with `TINHAT_UPDATE_URL`):
//!   { "version":"0.2.0", "url":"https://…/tinhat-0.2.0", "sha256":"…", "notes":"…",
//!     "url_bin":"https://…/tinhat-0.2.0-x86_64.bin", "sha256_bin":"…",       // U3: bare executable — preferred self-update payload
//!     "url_pkg":"https://…/….pkg.tar.zst", "sha256_pkg":"…" }                // U3: package archive — NEVER a self-update target
//!
//! CXX-Qt pattern mirrors settings.rs/room.rs: async work on the shared runtime, results
//! posted back to qproperties via qt_thread().queue (NOTIFY bindings are unreliable here,
//! so QML polls these into mirror props).

use core::pin::Pin;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qobject {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name] // snake_case Rust -> camelCase QML
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, app_version)] //       QML: appVersion — BUILD version (CARGO_PKG_VERSION); what the updater compares
        #[qproperty(QString, universal_version)] // QML: universalVersion — the shared feature-set version shown to users ("U0.1.0")
        #[qproperty(QString, build_hash)] //        QML: buildHash — short git hash of this build
        #[qproperty(QString, latest_version)] //    QML: latestVersion — version from the manifest
        #[qproperty(bool, update_available)] //     QML: updateAvailable
        #[qproperty(QString, release_notes)] //     QML: releaseNotes
        #[qproperty(QString, download_url)] //      QML: downloadUrl
        #[qproperty(QString, status)] //            QML: status — human-readable progress/result
        #[qproperty(bool, checking)] //             QML: checking
        #[qproperty(bool, downloading)] //          QML: downloading
        #[qproperty(bool, ready_to_restart)] //     QML: readyToRestart — update installed, restart to apply
        #[qproperty(bool, can_self_update)] //      QML: canSelfUpdate — false if pacman/system-managed (notify-only, use the package manager)
        #[qproperty(bool, portable_run)] //         QML: portableRun — Windows: a release bundle running OUTSIDE the install root (zip in Downloads)
        #[qproperty(QString, installed_version)] // QML: installedVersion — VERSION of the bundle at %LOCALAPPDATA%\TinHatApp\current ("" = none)
        #[qproperty(bool, install_prompt_due)] //   QML: installPromptDue — portable run and the first-run install prompt not yet answered
        #[qproperty(bool, installing)] //           QML: installing — install-to-apps copy in progress
        #[qproperty(QString, manual_update_hint)] // QML: manualUpdateHint — 10.0: what to do when the app can't update itself (per channel)
        #[qproperty(QString, channel)] //           QML: channel — 10.0: "flatpak" | "pacman-final" | "" (self-managed / Windows bundle)
        type UpdateController = super::UpdateControllerRust;

        /// QML: updater.checkForUpdates() — fetch the manifest + compare (non-blocking).
        #[qinvokable]
        fn check_for_updates(self: Pin<&mut UpdateController>);
        /// QML: updater.downloadAndApply() — download, verify sha256, atomically replace
        /// the running binary. Sets readyToRestart on success.
        #[qinvokable]
        fn download_and_apply(self: Pin<&mut UpdateController>);
        /// QML: updater.restart() — re-exec the (now-updated) binary in place.
        #[qinvokable]
        fn restart(self: Pin<&mut UpdateController>);
        /// QML: updater.installToApps() — Windows: copy this portable bundle into the
        /// self-updater layout, add shortcuts + an Apps & features entry, then relaunch
        /// from the installed copy (0.8.16). No-op elsewhere.
        #[qinvokable]
        fn install_to_apps(self: Pin<&mut UpdateController>);
        /// QML: updater.openInstalled() — Windows: launch the already-installed copy and
        /// quit this portable one.
        #[qinvokable]
        fn open_installed(self: Pin<&mut UpdateController>);
        /// QML: updater.dismissInstallPrompt() — remember "not now" so the first-run
        /// prompt never nags again (Settings keeps an Install button).
        #[qinvokable]
        fn dismiss_install_prompt(self: Pin<&mut UpdateController>);
    }

    impl cxx_qt::Threading for UpdateController {}
}

pub struct UpdateControllerRust {
    app_version: QString,
    universal_version: QString,
    build_hash: QString,
    latest_version: QString,
    update_available: bool,
    release_notes: QString,
    download_url: QString,
    status: QString,
    checking: bool,
    downloading: bool,
    ready_to_restart: bool,
    can_self_update: bool,
    manual_update_hint: QString,
    channel: QString,
    portable_run: bool,
    installed_version: QString,
    install_prompt_due: bool,
    installing: bool,
    // sha256 hex from the manifest, kept between check and apply (not a qproperty).
    expected_sha: String,
}

#[cfg(windows)]
fn portable_run() -> bool {
    crate::platform::is_portable_run()
}
#[cfg(not(windows))]
fn portable_run() -> bool {
    false
}
#[cfg(windows)]
fn installed_version() -> String {
    crate::platform::installed_bundle_version().unwrap_or_default()
}
#[cfg(not(windows))]
fn installed_version() -> String {
    String::new()
}
#[cfg(windows)]
fn install_prompt_due() -> bool {
    crate::platform::is_portable_run() && !crate::platform::install_prompt_seen()
}
#[cfg(not(windows))]
fn install_prompt_due() -> bool {
    false
}

/// Spawn `exe` as the successor of this process and exit (Windows relaunch pattern —
/// see restart()): the child inherits our stderr handle so the log reads as one file.
#[cfg(windows)]
fn relaunch_into(exe: &std::path::Path, why: &str) {
    if !exe.is_file() {
        eprintln!("[update] relaunch target missing: {}", exe.display());
        return;
    }
    eprintln!("[update] relaunching into {} ({why})", exe.display());
    let _ = std::process::Command::new(exe).env("TINHAT_RELAUNCH", why).spawn();
    std::process::exit(0);
}

/// Whether we may replace our own binary in place. False when the executable is under a
/// system-managed prefix (pacman/distro packaging) — there, updates must go through the
/// package manager (`pacman -Syu`), not an out-of-band self-replace that fights it.
/// True for user-writable installs (AppImage, ~/.local/bin, a dev build under $HOME).
/// 10.0: how this build is delivered. "flatpak" = inside the sandbox (the portal updates us),
/// "pacman-final" = the last pacman package (baked in by the PKGBUILD; it never updates again),
/// "" = a self-managed binary or the Windows bundle (the folder-swap updater).
pub(crate) fn in_flatpak() -> bool {
    std::path::Path::new("/.flatpak-info").exists()
}
pub(crate) fn channel() -> &'static str {
    if in_flatpak() { "flatpak" } else { option_env!("TINHAT_CHANNEL").unwrap_or("") }
}
/// The one line the banner shows when the app can't update itself, per channel.
pub(crate) fn manual_update_hint(portable: bool) -> String {
    match channel() {
        "flatpak" => "update it with Discover, or: flatpak update dev.tinhat.TinHat".to_string(),
        "pacman-final" => "this pacman build no longer updates — Tin Hat moved to Flatpak (install it from tinhat.dev)".to_string(),
        _ => {
            if cfg!(windows) {
                if portable {
                    "install it to your apps first (Settings → Install) — after that it updates itself".to_string()
                } else {
                    "download it again from tinhat.dev".to_string()
                }
            } else {
                "update it with your package manager".to_string()
            }
        }
    }
}

fn self_update_supported() -> bool {
    // Manual override (testing / if detection ever misfires): force package-managed mode.
    if std::env::var_os("TINHAT_PKGMODE").is_some() {
        return false;
    }
    // 10.0: a sandboxed Flatpak must never overwrite itself (the portal path updates it), and the
    // final pacman build is frozen on purpose.
    if in_flatpak() || channel() == "pacman-final" {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    #[cfg(not(windows))]
    {
        let p = exe.to_string_lossy();
        for sys in ["/usr/", "/opt/", "/bin/", "/sbin/", "/nix/store/"] {
            if p.starts_with(sys) {
                return false;
            }
        }
    }
    // Windows self-update is a whole-bundle folder swap (UPDATE_PATH.md §3) — it only
    // serves the portable layout (%LOCALAPPDATA%\TinHatApp\current\tinhat.exe).
    // A dev build under target\release must NOT advertise self-update: the manifest
    // points at a zip and there is no bundle to swap.
    #[cfg(windows)]
    if !crate::platform::is_bundle_install() {
        return false;
    }
    // Otherwise require the directory we'd rename within to be writable.
    let Some(dir) = exe.parent() else { return false };
    let probe = dir.join(".tinhat_write_probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

impl Default for UpdateControllerRust {
    fn default() -> Self {
        Self {
            app_version: QString::from(env!("CARGO_PKG_VERSION")),
            universal_version: QString::from(UNIVERSAL_VERSION),
            build_hash: QString::from(option_env!("TINHAT_BUILD_HASH").unwrap_or("dev")),
            latest_version: QString::default(),
            update_available: false,
            release_notes: QString::default(),
            download_url: QString::default(),
            status: QString::default(),
            checking: false,
            downloading: false,
            ready_to_restart: false,
            portable_run: portable_run(),
            installed_version: QString::from(&installed_version()),
            install_prompt_due: install_prompt_due(),
            installing: false,
            can_self_update: self_update_supported(),
            manual_update_hint: QString::from(&manual_update_hint(portable_run())),
            channel: QString::from(channel()),
            expected_sha: String::new(),
        }
    }
}

impl qobject::UpdateController {
    pub fn check_for_updates(mut self: Pin<&mut Self>) {
        if *self.checking() {
            return;
        }
        self.as_mut().set_checking(true);
        self.as_mut().set_status(QString::from("Checking for updates…"));
        // 10.0.2: the portal's poll clock starts when a monitor is registered — do it at
        // boot (this runs 4 s in), not on the click.
        if in_flatpak() {
            crate::flatpak::start_monitor();
        }
        let current = self.app_version().to_string();
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let result = fetch_manifest().await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_checking(false);
                match result {
                    Ok((ver, url, sha, notes)) => {
                        let avail = parse_ver(&ver) > parse_ver(&current);
                        o.as_mut().set_latest_version(QString::from(&ver));
                        o.as_mut().set_release_notes(QString::from(&notes));
                        o.as_mut().set_download_url(QString::from(&url));
                        o.as_mut().rust_mut().expected_sha = sha;
                        o.as_mut().set_update_available(avail);
                        o.as_mut().set_status(QString::from(if avail {
                            "Update available"
                        } else {
                            "You're on the latest version"
                        }));
                    }
                    Err(e) => {
                        eprintln!("[update] check failed: {e}");
                        o.as_mut().set_status(QString::from(&format!("Update check failed: {e}")));
                    }
                }
            });
        });
    }

    pub fn download_and_apply(mut self: Pin<&mut Self>) {
        if *self.downloading() || !*self.update_available() {
            return;
        }
        if !*self.can_self_update() {
            #[cfg(not(windows))]
            {
                if in_flatpak() {
                    // 10.1.1: the portal installs the new commit NOW (Update() never depended on its
                    // poll — see flatpak.rs) and narrates its progress; Restart relaunches into it.
                    let qt = self.qt_thread();
                    let done: crate::flatpak::Done = Box::new(move |ev: crate::flatpak::Event| {
                        let qt = qt.clone();
                        let _ = qt.queue(move |mut o| match ev {
                            crate::flatpak::Event::Status(line) => o.as_mut().set_status(QString::from(&line)),
                            crate::flatpak::Event::Finished(r) => {
                                o.as_mut().set_downloading(false);
                                match r {
                                    Ok(()) => {
                                        eprintln!("[update] applied through the Flatpak portal — restart to activate the new build");
                                        o.as_mut().set_ready_to_restart(true);
                                        o.as_mut().set_update_available(false);
                                        o.as_mut().set_status(QString::from("Update installed — restart to apply."));
                                    }
                                    Err(e) => o
                                        .as_mut()
                                        .set_status(QString::from(&format!("Update failed: {e} — or {}", manual_update_hint(false)))),
                                }
                            }
                        });
                    });
                    match crate::flatpak::request_apply(done) {
                        Ok(line) => {
                            self.as_mut().set_downloading(true);
                            self.as_mut().set_status(QString::from(&line));
                        }
                        Err(e) => self
                            .as_mut()
                            .set_status(QString::from(&format!("Update failed: {e} — or {}", manual_update_hint(false)))),
                    }
                    return;
                }
                let hint = manual_update_hint(false);
                self.as_mut().set_status(QString::from(&format!("This build is managed for you — {hint}.")));
            }
            #[cfg(windows)]
            {
                // A zip run (or a dev build): the folder-swap updater has nothing to swap.
                // The fix is one click away now — say so instead of naming a folder.
                let portable = *self.portable_run();
                self.as_mut().set_status(QString::from(if portable {
                    "This copy runs from a folder, so it can't update itself yet — use Install to your apps (Settings) once, and every update after that is automatic."
                } else {
                    "This is a development build — updates apply to installed copies only."
                }));
            }
            return;
        }
        self.as_mut().set_downloading(true);
        self.as_mut().set_status(QString::from("Downloading update…"));
        let url = self.download_url().to_string();
        let sha = self.rust().expected_sha.clone();
        let ver = self.latest_version().to_string();
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let result = do_download_and_apply(&url, &sha, &ver).await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_downloading(false);
                match result {
                    Ok(()) => {
                        // A successful update used to be LOG-INVISIBLE (only failures
                        // printed) — build-vintage archaeology on shipped logs needed
                        // format forensics to even notice one happened.
                        eprintln!("[update] applied — restart to activate the new build");
                        o.as_mut().set_ready_to_restart(true);
                        o.as_mut().set_update_available(false);
                        o.as_mut().set_status(QString::from("Update installed — restart to apply."));
                    }
                    Err(e) => {
                        eprintln!("[update] apply failed: {e}");
                        // os error 32 = the bundle folder is rename-locked. Our own CWD
                        // self-lock is fixed in early_startup; what remains is genuinely
                        // another holder (AV scan, second copy) — say so instead of
                        // looping the banner with no explanation.
                        let hint = if e.contains("os error 32") {
                            " — another program is holding the app folder (antivirus? a second copy?). Try again in a minute."
                        } else {
                            ""
                        };
                        o.as_mut().set_status(QString::from(&format!("Update failed: {e}{hint}")));
                    }
                }
            });
        });
    }

    pub fn install_to_apps(mut self: Pin<&mut Self>) {
        #[cfg(not(windows))]
        {
            self.as_mut()
                .set_status(QString::from("Installing to your apps is a Windows thing — this build is managed by your package manager."));
        }
        #[cfg(windows)]
        {
            if *self.installing() {
                return;
            }
            if !*self.portable_run() {
                self.as_mut().set_status(QString::from("Already installed."));
                return;
            }
            self.as_mut().set_installing(true);
            self.as_mut().set_status(QString::from("Installing Tin Hat to your apps… (copying)"));
            let qt = self.qt_thread();
            std::thread::spawn(move || {
                let result = crate::platform::install_from_portable();
                let _ = qt.queue(move |mut o| {
                    o.as_mut().set_installing(false);
                    match result {
                        Ok(exe) => {
                            eprintln!("[update] install-to-apps done: {}", exe.display());
                            o.as_mut().set_install_prompt_due(false);
                            o.as_mut().set_installed_version(QString::from(env!("CARGO_PKG_VERSION")));
                            o.as_mut().set_status(QString::from("Installed — opening the installed app…"));
                            relaunch_into(&exe, "install");
                        }
                        Err(e) => {
                            eprintln!("[update] install-to-apps failed: {e}");
                            o.as_mut().set_status(QString::from(&format!("Install failed: {e}")));
                        }
                    }
                });
            });
        }
    }

    pub fn open_installed(mut self: Pin<&mut Self>) {
        #[cfg(not(windows))]
        {
            let _ = &mut self;
        }
        #[cfg(windows)]
        {
            let exe = crate::platform::app_data_dir().join("current").join("tinhat.exe");
            if !exe.is_file() {
                self.as_mut().set_status(QString::from("No installed copy found — use Install to your apps."));
                return;
            }
            crate::platform::mark_install_prompt_seen();
            relaunch_into(&exe, "open-installed");
        }
    }

    pub fn dismiss_install_prompt(mut self: Pin<&mut Self>) {
        #[cfg(windows)]
        crate::platform::mark_install_prompt_seen();
        self.as_mut().set_install_prompt_due(false);
    }

    pub fn restart(self: Pin<&mut Self>) {
        if in_flatpak() {
            // 10.1.1: relaunch into the new deployment through the portal (flatpak-spawn
            // --latest-version runs the newest installed commit in a fresh sandbox the portal owns),
            // then quit. 12.0: the single-instance lock is shared by every instance of the Flatpak
            // and stays ours until this process is gone; the child (TINHAT_RELAUNCH=update) waits
            // for it, so the two never use the profile at the same time.
            match crate::flatpak::relaunch_latest(&[]) {
                Ok(()) => eprintln!("[update] flatpak: quitting — the relaunched instance runs the new deployment"),
                Err(e) => eprintln!("[update] flatpak: relaunch failed ({e}) — quitting; the next launch runs the new deployment"),
            }
            std::process::exit(0);
        }
        // Prefer the freshly-installed path captured by the apply step — after the swap,
        // current_exe() follows the renamed inode to `<exe>.bak` (the OLD build).
        let exe = installed_exe_path()
            .lock()
            .unwrap()
            .clone()
            .or_else(|| std::env::current_exe().ok());
        let Some(exe) = exe else { return };
        // U3 guard (c): never exec a swapped-in path that isn't a regular executable file
        // (an archive/garbage something managed to write there, a directory, a dangling
        // path). Refuse loudly instead of a confusing exec failure — or worse.
        let exe_ok = match std::fs::metadata(&exe) {
            Ok(m) => {
                #[cfg(unix)]
                let exec_bit = {
                    use std::os::unix::fs::PermissionsExt;
                    m.permissions().mode() & 0o111 != 0
                };
                #[cfg(not(unix))]
                let exec_bit = true;
                m.is_file() && exec_bit
            }
            Err(_) => false,
        };
        if !exe_ok {
            eprintln!(
                "[update] refusing restart: {} is not a regular executable file",
                exe.display()
            );
            self.set_status(QString::from(
                "Restart refused: the installed update is not a valid executable — reinstall from a fresh download.",
            ));
            return;
        }
        let args: Vec<String> = std::env::args().skip(1).collect();
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // exec replaces this process image with the updated binary (no orphan).
            let err = std::process::Command::new(&exe).args(&args).exec();
            eprintln!("[update] re-exec failed: {err}");
        }
        #[cfg(not(unix))]
        {
            // Marker env for the child: it inherits our stderr HANDLE (so it appends to
            // this very log — rotation is skipped by design), and the marker (a) prints
            // a visible session separator, (b) stops logship re-shipping the previous
            // session's log the parent already shipped (the duplicate-upload pairs seen
            // on every update day).
            eprintln!("[update] restarting into the updated build");
            let _ = std::process::Command::new(&exe)
                .args(&args)
                .env("TINHAT_RELAUNCH", "update")
                .spawn();
            std::process::exit(0);
        }
    }
}

/// Where the update feed lives: `latest.json` published with each release (the manifest shape documented at the top
/// of this file). selfhost (2026-10-21): the app has no release host of its own any more — the feed comes from the
/// app's GitHub releases. The repository is a deliberate choice (2026-10-21: the desktop app is `TinHatLabs/TinHat`;
/// the server moves to `TinHatLabs/TinHatServer`); the feed must be published there before a build ships
/// (`TINHAT_UPDATE_URL` overrides it for testing).
pub const UPDATE_FEED_URL: &str = "https://github.com/TinHatLabs/TinHat/releases/latest/download/latest.json";
/// Windows has its OWN channel (UPDATE_PATH.md §2): the url points at a bundle ZIP (not a bare binary) and the
/// sha256 is of that zip; its feed is published beside the Linux one.
pub const UPDATE_FEED_URL_WINDOWS: &str = "https://github.com/TinHatLabs/TinHat/releases/latest/download/latest-windows.json";

/// Manifest URL: env override (for testing) or the release feed ([`UPDATE_FEED_URL`] / [`UPDATE_FEED_URL_WINDOWS`]).
fn manifest_url() -> String {
    std::env::var("TINHAT_UPDATE_URL").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| {
        if cfg!(windows) { UPDATE_FEED_URL_WINDOWS.to_string() } else { UPDATE_FEED_URL.to_string() }
    })
}

/// (version, download_url, download_sha256, notes)
///
/// U3: the manifest may carry `url_bin`/`sha256_bin` (a BARE executable) and `url_pkg`/
/// `sha256_pkg` (the package archive) alongside the legacy `url`/`sha256`. The download-
/// and-apply path prefers `url_bin` (+`sha256_bin`) when present, else falls back to the
/// legacy pair. `url_pkg` is never a self-update payload — raw-writing an archive (the
/// pacman .pkg.tar.zst latest.json once pointed `url` at) over current_exe() bricks a
/// user-writable install; that near-miss is why url_bin exists. Windows keeps the legacy
/// pair unconditionally: its channel is a bundle ZIP by design (UPDATE_PATH.md §2), and
/// a bare-exe url_bin must never preempt the folder-swap payload there.
async fn fetch_manifest() -> Result<(String, String, String, String), String> {
    let url = manifest_url();
    // the app's User-Agent (proxy::http): GitHub's download host is friendlier to a named client
    let resp = crate::proxy::http(Some(std::time::Duration::from_secs(30)))
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let txt = resp.text().await.map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&txt).map_err(|e| e.to_string())?;
    pick_download(&v, cfg!(windows), std::env::var("TINHAT_UPDATE_URL").is_ok_and(|s| !s.trim().is_empty()))
}

/// The manifest's (version, download_url, download_sha256, notes) for this platform — the pure half of
/// [`fetch_manifest`], tested with the exact shapes publish-desktop-github writes. Linux prefers `url_bin` +
/// `sha256_bin` (U3; an empty sha256_bin is NOT a fallback trigger — do_download_and_apply hard-errors on a missing
/// sha); Windows takes `url` + `sha256` (the bundle zip). The download must be https:// unless the feed itself was
/// overridden for a test (`overridden`).
pub(crate) fn pick_download(v: &serde_json::Value, windows: bool, overridden: bool) -> Result<(String, String, String, String), String> {
    let get = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let ver = get("version");
    if ver.is_empty() {
        return Err("manifest has no version".into());
    }
    let (dl_url, dl_sha) = if windows {
        (get("url"), get("sha256"))
    } else {
        let url_bin = get("url_bin");
        if !url_bin.is_empty() {
            (url_bin, get("sha256_bin"))
        } else {
            (get("url"), get("sha256"))
        }
    };
    if !dl_url.is_empty() && !overridden && !dl_url.starts_with("https://") {
        return Err(format!("manifest names a download that isn't https ({dl_url}) — refusing it"));
    }
    Ok((ver, dl_url, dl_sha, get("notes")))
}

/// The UNIVERSAL feature-set version — what the user sees, shared by all three
/// clients (Windows native, Linux native, web). It is deliberately NOT the version
/// the updater compares.
///
/// Why the split: clients ALREADY IN USERS' HANDS (0.3.4) carry the old numeric
/// comparator below, and "U0.1.0" parses as (0,1,0) there — LOWER than 0.3.4 — so a
/// manifest advertising the U string would silently never offer them the upgrade.
/// The manifest therefore keeps a monotonic build version (CARGO_PKG_VERSION, 0.4.0
/// for U0.1.0) that every shipped client can compare, and the U string is display
/// only. UNIVERSAL.md records the mapping.
pub const UNIVERSAL_VERSION: &str = "U0.1.0";

/// Parse a "[U]X.Y.Z[-pre]" version into a comparable tuple (pre-release/build suffix
/// ignored). A leading `U` is tolerated and ranked ABOVE any non-U version, so that if
/// the manifest ever does carry U strings, clients new enough to have this code order
/// them correctly.
fn parse_ver(s: &str) -> (u64, u64, u64, u64) {
    let t = s.trim().trim_start_matches('v');
    let (universal, t) = match t.strip_prefix('U').or_else(|| t.strip_prefix('u')) {
        Some(rest) => (1, rest),
        None => (0, t),
    };
    let mut it = t
        .split('.')
        .map(|p| p.split(['-', '+']).next().unwrap_or("0").parse::<u64>().unwrap_or(0));
    (
        universal,
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

/// Download `url`, verify its sha256 against `expected_sha` (hex; a missing/empty
/// sha is a HARD error — unverified bytes are never installed), then
/// atomically replace the running executable. Linux lets us rename over a running binary's
/// path (the live process keeps its old inode); the new file is used on the next launch.
/// The path the update was installed to, captured BEFORE the backup rename. `restart()`
/// must exec THIS: after the swap, `current_exe()` (`/proc/self/exe`) follows the renamed
/// inode to `<exe>.bak`, so re-exec'ing it silently relaunches the OLD build.
fn installed_exe_path() -> &'static std::sync::Mutex<Option<std::path::PathBuf>> {
    static P: std::sync::OnceLock<std::sync::Mutex<Option<std::path::PathBuf>>> =
        std::sync::OnceLock::new();
    P.get_or_init(|| std::sync::Mutex::new(None))
}

/// U3 guard (a): the bytes about to be installed AS THE EXECUTABLE must begin with the
/// platform's executable magic — Linux: 0x7f 'E' 'L' 'F'; Windows: 'M' 'Z'. This is the
/// stop against a manifest whose chosen url points at an archive (e.g. the pacman
/// .pkg.tar.zst that latest.json's legacy `url` used to carry) being raw-written over
/// current_exe() on a user-writable install = bricked binary. Windows' normal channel
/// never takes the raw-write path (its payload is a bundle ZIP that install_update_zip
/// parses — a magic check of its own); the MZ arm covers any future bare-exe path there.
#[cfg_attr(windows, allow(dead_code))]
fn is_platform_executable(bytes: &[u8]) -> bool {
    #[cfg(windows)]
    {
        bytes.starts_with(b"MZ")
    }
    #[cfg(not(windows))]
    {
        bytes.starts_with(&[0x7f, b'E', b'L', b'F'])
    }
}

async fn do_download_and_apply(
    url: &str,
    expected_sha: &str,
    expected_version: &str,
) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    if url.is_empty() {
        return Err("no download url".into());
    }
    // the app's User-Agent, no overall timeout (a large download on a slow line); GitHub redirects to its download host
    let resp = crate::proxy::http(None)
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;

    // U3 guard (b): a manifest that names a download but carries no checksum is
    // malformed. The old behavior here — skip verification when sha256 is empty —
    // would install unverified bytes; HARD error instead.
    if expected_sha.trim().is_empty() {
        return Err("manifest has no sha256 for this download — refusing to install unverified bytes".into());
    }
    {
        let mut h = Sha256::new();
        h.update(&bytes);
        let got: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if !got.eq_ignore_ascii_case(expected_sha.trim()) {
            return Err(format!("checksum mismatch (expected {expected_sha}, got {got})"));
        }
    }

    // Install step is per-platform (UPDATE_PATH.md §6): Linux swaps the single binary;
    // Windows swaps the whole bundle FOLDER from the downloaded zip (§3 — replacing just
    // the exe next to stale Qt/GStreamer DLLs is a guaranteed ABI break).
    #[cfg(windows)]
    {
        // U3: the Windows payload is a bundle ZIP; install_update_zip parses it (its own
        // magic check), so a bad url can never be raw-written over the exe here. Still,
        // refuse obvious non-archives with a clear message instead of "open zip: ...".
        if !bytes.starts_with(b"PK") {
            return Err(
                "Update file is not a valid update bundle (zip) — refusing to install; grab a fresh download"
                    .into(),
            );
        }
        let promoted = crate::platform::install_update_zip(&bytes, expected_version)?;
        *installed_exe_path().lock().unwrap() = Some(promoted);
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = expected_version; // Linux manifests carry a bare binary; VERSION check is a bundle concept
        // U3 guard (a): refuse anything that isn't a real executable BEFORE touching disk.
        // Raw-writing an archive over current_exe() is a bricked install.
        if !is_platform_executable(&bytes) {
            return Err(
                "Update file is not an executable — refusing to install; update via your package manager or a fresh download"
                    .into(),
            );
        }
        // Prefer the path a PREVIOUS apply installed to: after that apply's rename,
        // current_exe() follows /proc/self/exe to `<exe>.bak`, so a second in-session update
        // would install to the .bak path and the real launch path would keep the stale build.
        let exe = match installed_exe_path().lock().unwrap().clone() {
            Some(p) => p,
            None => std::env::current_exe().map_err(|e| e.to_string())?,
        };
        let new_path = exe.with_extension("new");
        std::fs::write(&new_path, &bytes).map_err(|e| format!("write {}: {e}", new_path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&new_path, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| e.to_string())?;
        }
        // Move the running binary aside (kept as a backup; the live process is unaffected),
        // then move the new one into place. Restore on failure.
        let bak = exe.with_extension("bak");
        std::fs::rename(&exe, &bak).map_err(|e| format!("backup current exe: {e}"))?;
        if let Err(e) = std::fs::rename(&new_path, &exe) {
            let _ = std::fs::rename(&bak, &exe); // best-effort restore
            return Err(format!("install: {e}"));
        }
        *installed_exe_path().lock().unwrap() = Some(exe);
        Ok(())
    }
}

/// Headless self-test of the full Windows update cycle (TEST_PLAN.md §2.8) — run against
/// a LOCAL test manifest via `TINHAT_UPDATE_URL`, never the real channel. Two phases:
///
///   `TINHAT_UPDATE_TEST=apply`   fetch manifest → require newer → download → sha256 →
///                              folder swap → assert promoted VERSION → spawn the NEW
///                              exe in `verify` phase → exit (frees the old bundle).
///   `TINHAT_UPDATE_TEST=verify`  runs IN the promoted exe after the swap: sweep the
///                              leftover `backup\`/`staging\` (retrying while the old
///                              process dies), assert both gone + VERSION matches
///                              `TINHAT_UPDATE_EXPECT_VER`, print a verdict.
///
/// Exits the process in both phases (TINHAT_* gate tradition).
#[cfg(windows)]
pub fn headless_update_test(mode: &str) -> ! {
    let root = crate::platform::app_data_dir();
    let version_file = root.join("current").join("VERSION");
    match mode {
        "apply" => {
            let rc = crate::bridge::runtime().block_on(async {
                eprintln!("[updtest] manifest: {}", manifest_url());
                let (ver, url, sha, _notes) = match fetch_manifest().await {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("[updtest] VERDICT: FAIL — manifest fetch: {e}");
                        return 1;
                    }
                };
                // Compare the BUILD version, never the U string — see UNIVERSAL_VERSION.
                let current = env!("CARGO_PKG_VERSION");
                let avail = parse_ver(&ver) > parse_ver(current);
                eprintln!("[updtest] running={current} manifest={ver} update_available={avail}");
                if !avail {
                    eprintln!("[updtest] VERDICT: FAIL — manifest does not advertise a newer version");
                    return 1;
                }
                if let Err(e) = do_download_and_apply(&url, &sha, &ver).await {
                    eprintln!("[updtest] VERDICT: FAIL — apply: {e}");
                    return 1;
                }
                let promoted = std::fs::read_to_string(&version_file).unwrap_or_default();
                if promoted.trim() != ver {
                    eprintln!(
                        "[updtest] VERDICT: FAIL — promoted VERSION {:?} != manifest {ver:?}",
                        promoted.trim()
                    );
                    return 1;
                }
                eprintln!("[updtest] swap OK — promoted VERSION={ver}; spawning verify phase in the NEW exe");
                let new_exe = root.join("current").join("tinhat.exe");
                match std::process::Command::new(&new_exe)
                    .env("TINHAT_UPDATE_TEST", "verify")
                    .env("TINHAT_UPDATE_EXPECT_VER", &ver)
                    // The dying parent may still hold the single-instance lock for a
                    // beat — the verify child must not lose that race (test-only bypass,
                    // TEST_PLAN.md §0 rule 3; no call is involved).
                    .env("TINHAT_NO_SINGLE_INSTANCE", "1")
                    .spawn()
                {
                    Ok(_) => 0,
                    Err(e) => {
                        eprintln!("[updtest] VERDICT: FAIL — spawn of promoted exe: {e}");
                        1
                    }
                }
            });
            // Exit NOW so the old (renamed-to-backup) bundle unlocks for the child's sweep.
            std::process::exit(rc);
        }
        "verify" => {
            let expect = std::env::var("TINHAT_UPDATE_EXPECT_VER").unwrap_or_default();
            // The old process may still be dying — retry the sweep for up to ~10s.
            let backup = root.join("backup");
            let staging = root.join("staging");
            for _ in 0..20 {
                if !backup.exists() && !staging.exists() {
                    break;
                }
                crate::platform::cleanup_after_update();
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            let ver = std::fs::read_to_string(&version_file).unwrap_or_default().trim().to_string();
            let ok = !backup.exists() && !staging.exists() && (expect.is_empty() || ver == expect);
            eprintln!(
                "[updtest] VERDICT: {} — running from promoted bundle: VERSION={ver} (expect {expect}); backup swept={}; staging swept={}",
                if ok { "PASS" } else { "FAIL" },
                !backup.exists(),
                !staging.exists()
            );
            std::process::exit(if ok { 0 } else { 1 });
        }
        other => {
            eprintln!("[updtest] VERDICT: FAIL — unknown mode {other:?} (use apply|verify)");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // the two manifests exactly as the release tool (publish-desktop-github) writes them (its python block)
    const BASE: &str = "https://github.com/TinHatLabs/TinHat/releases/download/v11.4.0";
    fn latest() -> serde_json::Value {
        json!({ "version": "11.4.0", "url": format!("{BASE}/tinhat-11.4.0-x86_64.bin"), "sha256": "aa",
                "url_bin": format!("{BASE}/tinhat-11.4.0-x86_64.bin"), "sha256_bin": "aa", "notes": "Notes." })
    }
    fn latest_windows() -> serde_json::Value {
        json!({ "version": "11.4.0", "url": format!("{BASE}/tinhat-11.4.0-win64.zip"), "sha256": "bb",
                "installer": format!("{BASE}/TinHat-Setup-11.4.0.exe"), "sha256_installer": "cc", "notes": "Notes." })
    }

    #[test]
    fn the_feed_is_the_github_release_of_tinhatlabs_tinhat() {
        assert_eq!(UPDATE_FEED_URL, "https://github.com/TinHatLabs/TinHat/releases/latest/download/latest.json");
        assert_eq!(UPDATE_FEED_URL_WINDOWS, "https://github.com/TinHatLabs/TinHat/releases/latest/download/latest-windows.json");
        for u in [UPDATE_FEED_URL, UPDATE_FEED_URL_WINDOWS] {
            assert!(u.starts_with("https://github.com/TinHatLabs/"), "{u}"); // never a self-run release host
        }
    }

    #[test]
    fn the_tools_manifests_give_each_platform_its_payload() {
        // Linux: the bare binary (url_bin + sha256_bin)
        assert_eq!(
            pick_download(&latest(), false, false).unwrap(),
            ("11.4.0".into(), format!("{BASE}/tinhat-11.4.0-x86_64.bin"), "aa".into(), "Notes.".into())
        );
        // Windows: the bundle zip (url + sha256), never the installer
        assert_eq!(
            pick_download(&latest_windows(), true, false).unwrap(),
            ("11.4.0".into(), format!("{BASE}/tinhat-11.4.0-win64.zip"), "bb".into(), "Notes.".into())
        );
        // a legacy manifest without url_bin still works on Linux
        let legacy = json!({ "version": "1.0.0", "url": "https://x.test/a.bin", "sha256": "dd" });
        assert_eq!(pick_download(&legacy, false, false).unwrap().1, "https://x.test/a.bin");
        // an empty sha256_bin is not a fallback to url/sha256 (the download step refuses unverified bytes)
        let no_sha = json!({ "version": "1.0.0", "url": "https://x.test/a", "sha256": "dd", "url_bin": "https://x.test/b", "sha256_bin": "" });
        assert_eq!(pick_download(&no_sha, false, false).unwrap().2, "");
    }

    #[test]
    fn a_manifest_without_a_version_or_with_a_plain_http_download_is_refused() {
        assert!(pick_download(&json!({ "url": "https://x.test/a" }), false, false).is_err());
        assert!(pick_download(&json!("x"), false, false).is_err());
        let http = json!({ "version": "1.0.0", "url_bin": "http://x.test/a", "sha256_bin": "aa" });
        assert!(pick_download(&http, false, false).is_err());
        // … unless the feed itself was overridden for a test (TINHAT_UPDATE_URL, a local server)
        assert_eq!(pick_download(&http, false, true).unwrap().1, "http://x.test/a");
    }

    #[test]
    fn versions_compare_as_before() {
        assert!(parse_ver("11.4.0") > parse_ver("11.3.9"));
        assert!(parse_ver("v11.4.0") == parse_ver("11.4.0"));
        assert!(parse_ver("11.4.0-rc1") == parse_ver("11.4.0"));
    }
}
