// Tin Hat native app entry point.
// Rust owns the Qt application + QML engine (no generic `qml6` runtime anymore);
// the Rust core is exposed to QML through the CXX-Qt bridges in `bridge`/`stream`/`room`.
// Windows: GUI subsystem — a double-click/shortcut launch must not spawn a console
// window next to the app. Diagnostics still reach a dev terminal (parent-console
// attach) or the log file: platform::redirect_stderr_to_log decides at startup.
#![cfg_attr(windows, windows_subsystem = "windows")]
pub mod activity; // "What we're playing" per-room activity (dev.tinhat.activity)
pub mod afk_ctl; // AfkController — #7 BRB/AFK presence in a call (Matrix reactions/state)
pub mod assets; // bundled app assets (icon layers + chimes) -> runtime dir
pub mod callwatch; // 10.0: incoming-call edges per sync -> LoginController signals
pub mod quietrules; // 10.3: account push rules that keep call joins / room housekeeping out of the unread counts
pub mod gameservers_ctl; // 11.1: GameServersController — the Game servers widget's data (docs/GAMESERVERS.md)
pub mod justme_ctl; // widgets W2: JustMeController — "Is it just me?" (WIDGETS.md §20)
pub mod memories_ctl; // widgets W2: MemoriesController — the Memories widget's data (WIDGETS.md §21)
pub mod gamenight_ctl; // widgets W2: GameNightController — Game night's rules + my Reel credit (WIDGETS.md §23)
pub mod invites; // 10.4: InviteController — registration invite codes (docs/INVITES.md)
pub mod guests; // selfhost: the guest room's shared rules (the web's features/guests/model.ts; docs/GUESTS.md)
pub mod guests_ctl; // selfhost: GuestsController — Settings → Guests, the Guests room's bar, the waiting notices (docs/GUESTS.md)
pub mod flatpak; // 10.0: Flatpak portal update path
pub mod audio_prefs; // AudioPrefsController — per-(category,user) call volume/mute (#5)
pub mod audioapps; // per-application audio share sources (PipeWire nodes / WASAPI sessions) (0.8.22)
pub mod bridge; // LoginController — Matrix login/session/room-list
pub mod call_ctl; // CallController — full call participation (join/mic/screen)
pub mod cam_gltest; // CamGlTest — TINHAT_CAM_GLTEST camera GL-upload (qml6glsink) self-test harness
pub mod colorscheme; // full-app theming via generated KDE color schemes (KColorSchemeManager)
pub mod theme_gen; // themes (C1): the Th QML singleton's shape (build.rs generates qml/Th.qml from it)
pub mod theme_rt; // themes (C1): the theme runtime — which themes exist, the engine, the process-wide active theme
pub mod theme_ctl; // themes (C1): ThemeController — the engine → the Th singleton (AppCore.applyTheme)
pub mod theme_editor; // 11.0 step 5: ThemeEditorController — the theme editor, share cards, the Pick tool
pub mod theme_lib; // themes (1b): the built-in theme library (contract themes-library.json, embedded)
pub mod theme_store; // themes (1b): your own + kept themes, the account theme, this device's choice (THEMES-SYNC.md)
pub mod callstats_ctl; // widgets (step 2): CallStatsController — the Call stats widget's live data (streamer callstats.rs)
pub mod widgets_ctl; // widgets (step 2): WidgetsController — call history, Up next's events, the shared formatters
pub mod mentions_ctl; // widgets W1: MentionsController — this device's list of recent highlights (Mentions widget)
pub mod note_ctl; // widgets W1: NoteController — the space's Sticky note (dev.tinhat.note, edit_space_note)
pub mod dice_ctl; // widgets W3: DiceController — rolling (OS CSPRNG), the private rolls, the Dice widget's history
pub mod polls_ctl; // widgets W3: PollsController + the poll card's JSON (the shared polls logic for QML)
pub mod lowgpu_ctl; // step 3 (GPU.md): LowGpuController — the low-GPU contract's rules for QML (tinhat_widgets::lowgpu)
pub mod layout_ctl; // islands (C1): LayoutController — the layout engine (crates/layout) as JSON in / JSON out; 11.0 edit mode: the session + the synced layouts' QML face
pub mod layout_store; // 11.0 edit mode: your layouts across your devices (dev.tinhat.layouts, EDITMODE-UI §16)
pub mod actions_ctl; // 11.0 step 3 (buttons): ActionsController — the actions engine (crates/actions) as JSON in / JSON out
pub mod dm_target; // 2026-09-29: the New DM field — a plain username → @name:<our server> (shared vectors with the web)
pub mod new_room; // 2026-10-23: what New room starts with — encrypted rooms, unencrypted spaces (shared vectors with the web)
pub mod diagnostics; // opt-in, redacted, user-inspectable bug report (never auto-sent)
pub mod logship; // opt-OUT scrubbed log upload + problem reports, to the user's OWN server (consent popup; Settings → Privacy)
pub mod e2ee_keys; // Element Call media-key (E2EE) exchange over Matrix
pub mod keyrules; // the media-key trust rules (pure; = the web's features/calls/keyrules.ts)
pub mod key_intake; // every Olm-decrypted media key goes through keyrules before the keyring
pub mod embeds; // inline click-to-play media-embed URL parser (ported from the earlier web client)
pub mod linkpreview; // OpenGraph link-preview cards via the homeserver's /media/preview_url (9.3)
pub mod floating; // FloatingController — #6 floating chat-bubble registry + metadata
pub mod gif; // GifController — in-chat Giphy picker (via the server's Tin Hat API)
pub mod appstate; // shared custom room-state-event read/write/PL-lower helpers
pub mod nav; // NavController — corner-dock navigation (spaces -> rooms drill-down)
pub mod presence; // 10.6: PresenceController — opt-in Online/Away/In a call + the desktop's "at the computer" pings (docs/REACH.md)
pub mod presence_model; // 10.6: the pure presence rules (cadence, idle → state, share transitions, reader map, §3.4 consent)
pub mod pipvideo; // PipVideoItem — PiP pop-out CPU video surface (no GstGL; NVIDIA-safe)
pub mod platform; // OS-specific glue: data paths, file:// URLs, bundle env, Windows update swap
pub mod notify; // OS desktop notifications (org.freedesktop.Notifications) for new highlights
pub mod joinsound; // per-user join sounds published over the soundboard bridge (QOL 1.5.2)
pub mod livekit; // OpenID -> lk-jwt-service LiveKit token (used by stream)
pub mod proxy; // the user's server's Tin Hat API client (gif/soundboard/watch — keys stay server-side; the base from its well-known)
pub mod server_offer; // selfhost: what the user's server offers (dev.tinhat.server in its well-known): API base, features, recorder, call service
pub mod password; // selfhost: a forgotten password (the server's owner resets it: account.password_reset) and Change password (UIA)
#[cfg(windows)]
pub mod qvideo; // FFI to the QVideoSink prototype shim + TINHAT_QVS_TEST (Windows video tail)
pub mod room; // RoomController — timeline + send (M3 chat)
pub mod rtc; // MatrixRTC: detect active call + resolve the real LiveKit focus
pub mod session; // process-global logged-in Client, shared between controllers
pub mod settings; // SettingsController — account profile / devices / security state
pub mod settings_search; // the Settings window's search: the shared index + matcher (contract SETTINGS-WINDOW.md §4)
pub mod signout; // 2026-10-20: what signing out forgets of the account on this device (as the web's doLogout)
pub mod soundboard; // SoundboardController — room + personal sounds, web search (dev.tinhat.soundboard)
pub mod status_ctl; // per-person "looking to play" status (dev.tinhat.userstatus)
pub mod stream; // StreamController — NVENC screen-share publish into a call
pub mod update; // UpdateController — in-app / startup update check + self-replace
pub mod video; // FFI to the C++ qml6glsink bridge (native remote-video tiles)
pub mod vodsvc; // Memories — the headless call-recorder service (0.8.7; the recorder service host only)
pub mod medialinks; // Media Party link parsing (pure; twin of the web links.ts)
pub mod watchparty; // WatchPartyController — queue/playback-clock/viewer-presence + sync + search
pub mod webengine; // FFI to the QtWebEngineQuick::initialize() shim (inline iframe embeds)
pub mod clipboard; // FFI to the clipboard shim — what Ctrl+V attaches (9.3.2)
pub mod boot; // BootController — startup window stages while the GUI thread is still busy (0.8.24)
pub mod calendar; // CalendarController — squad calendar: events/RSVP/proposals/rules in the space (0.8.26)
pub mod games; // GamesController — Game Reel: "what do we play tonight" picker in the space (9.0)
pub mod games_model;
pub mod roles_model; // RBAC: roles + permissions pure model (docs/RBAC.md)
pub mod roles; // RBAC: async role helpers (container lookup, effective permissions)
pub mod space; // SpaceController — space settings: overview/members/roles/rooms (docs/RBAC.md)
pub mod moved; // "Move to your own domain": relations of a moved server's events by today's ids (shared with the web)
pub mod moderation; // RBAC: call-moderation client for /api/moderation/call // Game Reel pure fairness model (pool tiers, shares, regret credit, verifiable winner) — no Qt
pub mod iconpack; // IconPackController — uniform (animated) icon pack discovery + static theme export (0.8.26)
#[cfg(target_os = "linux")]
pub mod sas_test; // TINHAT_SASTEST headless two-device emoji-verification harness
pub mod sas_policy; // emoji-verification rules shared word for word with Tin Hat Web (S1 start policy, S6 cancel notes) — no Qt
pub mod utd; // selfhost/firstrun §3: the words for a message this device can't read, shared with Tin Hat Web (utd.ts)
pub mod sas_driver; // Qt-free emoji-verification driver (app + TINHAT_SASTEST harness run the same code)

use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QString, QUrl};

/// Redirect stderr (fd 2) to a log file so diagnostics survive any launch method.
/// Keep it simple: truncate per run; print the path to stdout (the real terminal).
/// TINHAT_NO_LOG_REDIRECT=1 keeps stderr as-is — used by forked gate children
/// (TINHAT_SPEAKERTEST_CHILD), which inherit the parent's already-redirected fd 2; a
/// second File::create here would TRUNCATE the parent's log mid-run.
#[cfg(unix)]
fn redirect_diagnostics_to_file() {
    use std::os::unix::io::AsRawFd;
    if std::env::var_os("TINHAT_NO_LOG_REDIRECT").is_some() {
        crate::platform::set_live_log(None);
        return;
    }
    // 0.8.29: per-user state dir, mode 0700 — never /tmp again (uid-less paths
    // on sticky /tmp failed silently for any uid mismatch and let another local user
    // plant a file that got uploaded under this user's Matrix id).
    let dir = crate::platform::log_dir();
    let _ = std::fs::create_dir_all(&dir);
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let path = dir.join("tinhat.log");
    let old = dir.join("tinhat.old.log");
    // Rotate instead of destroy: the previous session's log is the evidence for any
    // "it broke, I restarted, then filed a report" bug (an earlier build shipped a useless log
    // for exactly this reason). Safe here: the single-instance guard already ran, so
    // a rejected second launch can't steal the live instance's log out from under it.
    let _ = std::fs::rename(&path, &old);
    match std::fs::File::create(&path) {
        Ok(file) => {
            unsafe {
                libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO);
            }
            std::mem::forget(file); // keep the fd open for the process lifetime
            crate::platform::set_live_log(Some(path.clone()));
            println!("Tin Hat: diagnostics -> {}", path.display());
        }
        Err(e) => {
            crate::platform::set_live_log(None);
            eprintln!(
                "[main] could not open {} for diagnostics ({e}) — stderr stays where it was; \
                 bug reports from this run will say the live log is unavailable",
                path.display()
            );
        }
    }
}

/// Windows: stderr stays on the console for now. The real redirect
/// (%LOCALAPPDATA%\TinHatApp\logs + SetStdHandle/_dup2, PLATFORM_AUDIT.md §9) lands
/// with the platform:: module — dev builds run from a terminal until then.
#[cfg(windows)]
fn redirect_diagnostics_to_file() {
    // Handled in platform::early_startup (before this runs): a parent-provided
    // redirect handle is kept, a parent console is attached, and otherwise stderr
    // goes to %LOCALAPPDATA%\TinHatApp\logs\tinhat.log with rotation.
    eprintln!("[main] diagnostics routing decided at early-startup (console / redirect / log file)");
}

/// Print a one-screen verdict for an [`e2ee_loopback`] run so a self-test is decisive
/// at a glance (no log archaeology).
fn print_loopback(label: &str, r: &tinhat_streamer::call::LoopbackReport) {
    let identity_honored = r.pub_identity != r.sub_identity && !r.pub_identity.is_empty();
    eprintln!("==== LOOPBACK {label} SUMMARY ====");
    eprintln!("  pub_identity = {:?}", r.pub_identity);
    eprintln!("  sub_identity = {:?}", r.sub_identity);
    eprintln!("  lk-jwt honored device_id (distinct identities) = {identity_honored}");
    eprintln!(
        "  subscribed={} frames_decoded={} luma_peak={}",
        r.subscribed, r.frames_decoded, r.luma_peak
    );
    eprintln!("  e2ee_states = {:?}", r.e2ee_states);
    let verdict = if !identity_honored {
        "INVALID — lk-jwt ignored device_id (pub==sub identity); loopback can't run two participants"
    } else if label == "POSITIVE" {
        if r.frames_decoded > 0 && r.luma_peak > 0 {
            "PASS — peer key installed under the real identity DECRYPTS + DECODES"
        } else {
            "FAIL — key installed but nothing decoded (inspect e2ee_states: MissingKey=identity/index, DecryptionFailed=bytes/KDF)"
        }
    } else if r.frames_decoded == 0 {
        "PASS — no key => no decode (E2EE is genuinely active)"
    } else {
        "SUSPECT — decoded WITHOUT a key (E2EE may be off!)"
    };
    eprintln!("  VERDICT: {verdict}");
    eprintln!("=================================");
}

/// The single-instance lock for the process lifetime (see acquire_single_instance_lock). It is
/// never released early: the kernel drops the flock when this process is gone, and a relaunched
/// successor (Flatpak restart after an update, TINHAT_RELAUNCH set) waits for exactly that, so
/// the two never use the profile at the same time.
static INSTANCE_LOCK: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);

/// How long a Flatpak relaunch (TINHAT_RELAUNCH set) waits for its predecessor to exit and
/// free the shared lock before it concludes another instance is running.
#[cfg(unix)]
const RELAUNCH_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Single-instance guard: hold an exclusive `flock` on a per-user runtime file so a SECOND
/// GUI launch can't create a conflicting Matrix/LiveKit session under the SAME `device_id`.
/// Two instances mint the identical `user:device` LiveKit identity — the SFU can't tell them
/// apart, and a ghost participant from one wedges the other's call rejoin ("connecting…"
/// forever). `flock` releases automatically when the process dies (even on crash), so there's
/// no stale-lock risk. Keep the returned `File` alive for the process lifetime.
/// `TINHAT_NO_SINGLE_INSTANCE=1` bypasses the guard (e.g. to intentionally run a 2nd copy).
/// The lock file is `$XDG_RUNTIME_DIR/tinhat.lock` (else `/run/user/<uid>/tinhat.lock`); inside a
/// Flatpak it is `$XDG_RUNTIME_DIR/app/<app id>/tinhat.lock` (see [`flatpak_shared_lock_dir`]).
#[cfg(unix)]
fn acquire_single_instance_lock() -> Option<std::fs::File> {
    if std::env::var_os("TINHAT_NO_SINGLE_INSTANCE").is_some() {
        return None;
    }
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() }))
        });
    let mut shared = false;
    let dir = if update::in_flatpak() {
        let info = std::fs::read_to_string("/.flatpak-info").unwrap_or_default();
        let env_id = std::env::var("FLATPAK_ID").ok();
        match flatpak_shared_lock_dir(&runtime_dir, &info, env_id.as_deref(), |p| p.is_dir()) {
            Ok(d) => {
                shared = true;
                d
            }
            Err(why) => {
                eprintln!("[main] flatpak: {why}; the single-instance lock covers this sandbox only");
                runtime_dir
            }
        }
    } else {
        runtime_dir
    };
    let path = dir.join("tinhat.lock");
    // A Flatpak restart starts us in a new sandbox while the previous instance still holds the
    // shared lock until it is gone (flatpak.rs relaunch_latest): wait for it, bounded, instead of
    // refusing at once. Any other launch (no TINHAT_RELAUNCH) is refused at once, as before.
    let wait_until = (shared && std::env::var_os("TINHAT_RELAUNCH").is_some())
        .then(|| std::time::Instant::now() + RELAUNCH_LOCK_WAIT);
    let mut waited = false;
    let file = loop {
        match lock_file_exclusive(&path) {
            Ok(None) if wait_until.is_some_and(|t| std::time::Instant::now() < t) => {
                if !waited {
                    eprintln!("[main] relaunch: waiting for the previous instance to exit");
                    waited = true;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
                continue;
            }
            r => break r,
        }
    };
    let file = match file {
        Ok(Some(f)) => f,
        Ok(None) => {
            eprintln!("[main] another Tin Hat instance is already running — exiting this one.");
            eprintln!("[main] (set TINHAT_NO_SINGLE_INSTANCE=1 to force a second instance.)");
            println!("Tin Hat is already running.");
            // 10.1 (TH-PKG-09): the .desktop entry has Terminal=false, so that line reaches nobody —
            // a second click on the app icon used to do nothing visible at all. Best-effort toast.
            let _ = std::process::Command::new("notify-send")
                .args(["-a", "Tin Hat", "-i", "dev.tinhat.TinHat", "Tin Hat is already running", "It's open in another window — check your taskbar."])
                .spawn();
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("[main] single-instance lock unavailable ({e}); continuing without guard");
            return None;
        }
    };
    let scope = if shared { " (shared by every instance of this Flatpak app)" } else { "" };
    eprintln!("[main] single-instance lock acquired{scope}");
    Some(file)
}

/// Open (create) `path` and take an exclusive non-blocking `flock` on it. `Ok(Some)` = we hold
/// it (keep the `File` alive), `Ok(None)` = another process holds it, `Err` = the file can't be
/// opened. The fd is close-on-exec (std's default), so a spawned helper never inherits the lock.
#[cfg(unix)]
fn lock_file_exclusive(path: &std::path::Path) -> std::io::Result<Option<std::fs::File>> {
    use std::os::unix::io::AsRawFd;
    let file = std::fs::OpenOptions::new().create(true).write(true).open(path)?;
    // LOCK_NB: fail immediately (don't block) if another instance already holds it.
    let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    Ok(if ret == 0 { Some(file) } else { None })
}

/// Where a Flatpak instance keeps its single-instance lock. Inside the sandbox `$XDG_RUNTIME_DIR`
/// is a private tmpfs of EACH instance, so a lock file there never meets the next instance's: a
/// second Flatpak copy started on the same profile (one crypto store, one device_id). Flatpak
/// bind-mounts the host's `$XDG_RUNTIME_DIR/app/<app id>` into every instance of the same app,
/// so the lock goes there. The id is the running sandbox's own ([`flatpak_app_id`]), never a
/// fixed name. `Err` = why the caller falls back to the per-sandbox `$XDG_RUNTIME_DIR`.
#[cfg(unix)]
fn flatpak_shared_lock_dir(
    runtime_dir: &std::path::Path,
    flatpak_info: &str,
    env_id: Option<&str>,
    is_dir: impl Fn(&std::path::Path) -> bool,
) -> Result<std::path::PathBuf, String> {
    if !runtime_dir.is_absolute() {
        return Err(format!("XDG_RUNTIME_DIR {:?} is not an absolute path", runtime_dir));
    }
    let id = flatpak_app_id(flatpak_info, env_id)
        .ok_or_else(|| "no valid app id in /.flatpak-info or FLATPAK_ID".to_string())?;
    let dir = runtime_dir.join("app").join(id);
    if is_dir(&dir) {
        Ok(dir)
    } else {
        Err(format!("the shared app runtime folder {} is missing", dir.display()))
    }
}

/// The running Flatpak's app id: the `name` key of `[Application]` in `/.flatpak-info` (Flatpak
/// writes that file; it is read-only inside the sandbox). Only when the file names no app is
/// `$FLATPAK_ID` used; a name in the file that isn't a valid Flatpak name gives None (the file
/// decides, the environment never overrides it). A valid name is one safe path component.
#[cfg(unix)]
fn flatpak_app_id(flatpak_info: &str, env_id: Option<&str>) -> Option<String> {
    let mut section = "";
    let mut from_info = None;
    for line in flatpak_info.lines() {
        let l = line.trim();
        if let Some(name) = l.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = name.trim();
        } else if section == "Application" {
            if let Some((k, v)) = l.split_once('=') {
                if k.trim() == "name" {
                    from_info = Some(v.trim());
                    break;
                }
            }
        }
    }
    let id = from_info.or_else(|| env_id.map(str::trim))?;
    valid_flatpak_app_id(id).then(|| id.to_string())
}

/// Flatpak's own name rule (flatpak_is_valid_name, common/flatpak-ref-utils.c): 1–255 bytes, at
/// least three dot-separated elements, each non-empty, of `[A-Za-z0-9_]`, not starting with a
/// digit; `-` is allowed only in the last element (there also as its first character).
#[cfg(unix)]
fn valid_flatpak_app_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('.').collect();
    if id.is_empty() || id.len() > 255 || parts.len() < 3 {
        return false;
    }
    let last = parts.len() - 1;
    parts.iter().enumerate().all(|(i, p)| {
        let dash = i == last;
        let mut b = p.bytes();
        let first_ok = b.next().is_some_and(|c| c.is_ascii_alphabetic() || c == b'_' || (dash && c == b'-'));
        first_ok && b.all(|c| c.is_ascii_alphanumeric() || c == b'_' || (dash && c == b'-'))
    })
}

#[cfg(all(test, unix))]
mod single_instance_tests {
    use super::*;
    use std::path::{Path, PathBuf};

    const INFO: &str = "[Application]\nname=dev.tinhat.TinHat\nruntime=runtime/org.kde.Platform/x86_64/6.11\n\n\
                        [Instance]\ninstance-id=1234567890\napp-path=/var/lib/flatpak/app/dev.tinhat.TinHat/x86_64/master/abc/files\n";

    #[test]
    fn app_id_comes_from_the_application_section() {
        assert_eq!(flatpak_app_id(INFO, None).as_deref(), Some("dev.tinhat.TinHat"));
        // /.flatpak-info wins over the environment (the file is Flatpak's own, the env is not)
        assert_eq!(flatpak_app_id(INFO, Some("org.example.Other")).as_deref(), Some("dev.tinhat.TinHat"));
        // a name= in another section is not the app id
        let other = "[Runtime]\nname=org.kde.Platform\n[Instance]\nname=x.y\n";
        assert_eq!(flatpak_app_id(other, None), None);
        // no file (or no [Application]): FLATPAK_ID
        assert_eq!(flatpak_app_id("", Some("dev.tinhat.TinHat")).as_deref(), Some("dev.tinhat.TinHat"));
        assert_eq!(flatpak_app_id(other, Some(" dev.tinhat.TinHat ")).as_deref(), Some("dev.tinhat.TinHat"));
        assert_eq!(flatpak_app_id("", None), None);
        // tolerant of CRLF and spaces around the section header, the key and the value
        let crlf = " [ Application ] \r\nname = dev.tinhat.TinHat \r\n";
        assert_eq!(flatpak_app_id(crlf, None).as_deref(), Some("dev.tinhat.TinHat"));
        // a key that only starts with "name" is another key
        assert_eq!(flatpak_app_id("[Application]\nnamespace=a.b.c\n", None), None);
    }

    #[test]
    fn unsafe_or_invalid_ids_are_refused() {
        for bad in [
            "", ".", "..", "single", "a.b", "dev.TinHat", "../../tmp", "../a.b.c", "dev/tinhat.TinHat",
            "dev.tinhat/x.TinHat", "dev..tinhat.TinHat", ".dev.tinhat.TinHat", "dev.tinhat.TinHat.",
            "-dev.tinhat.TinHat", "dev.1tinhat.TinHat", "dev.tinhat.1TinHat", "org.ex-ample.TinHat",
            "dev.tin hat.TinHat", "dev.tinhat.TinHat\0x", "dev.tinhat.Tin~Hat",
        ] {
            assert_eq!(flatpak_app_id("", Some(bad)), None, "{bad:?}");
            assert_eq!(flatpak_app_id(&format!("[Application]\nname={bad}\n"), Some("dev.tinhat.TinHat")), None, "{bad:?}");
        }
        assert!(valid_flatpak_app_id(&format!("a.b.{}", "c".repeat(251))));
        assert!(!valid_flatpak_app_id(&format!("a.b.{}", "c".repeat(252))));
        for good in [
            "dev.tinhat.TinHat", "org.kde.Platform", "a.b.c", "_x.y.z-z", "com.example.App_2",
            "org.example.-TinHat", "dev.tinhat.Tin-Hat", "org.example.app.Sub_9",
        ] {
            assert!(valid_flatpak_app_id(good), "{good:?}");
        }
    }

    #[test]
    fn the_lock_goes_into_the_shared_app_folder() {
        let rt = Path::new("/run/user/1000");
        let want = PathBuf::from("/run/user/1000/app/dev.tinhat.TinHat");
        assert_eq!(flatpak_shared_lock_dir(rt, INFO, None, |p| p == want.as_path()), Ok(want.clone()));
        // two instances of the same app compute the same folder; another app gets its own
        assert_eq!(flatpak_shared_lock_dir(rt, "", Some("dev.tinhat.TinHat"), |_| true), Ok(want));
        assert_eq!(
            flatpak_shared_lock_dir(rt, "", Some("org.example.Other"), |_| true),
            Ok(PathBuf::from("/run/user/1000/app/org.example.Other"))
        );
        // the id names exactly one folder below app/ (no traversal out of it)
        assert!(flatpak_shared_lock_dir(rt, "", Some("../../../tmp"), |_| true).is_err());
        // the shared folder missing, no id, or a relative runtime dir: Err (= per-sandbox fallback)
        assert!(flatpak_shared_lock_dir(rt, INFO, None, |_| false).is_err());
        assert!(flatpak_shared_lock_dir(rt, "", None, |_| true).is_err());
        assert!(flatpak_shared_lock_dir(Path::new(""), INFO, None, |_| true).is_err());
        assert!(flatpak_shared_lock_dir(Path::new("run/user/1000"), INFO, None, |_| true).is_err());
    }

    #[test]
    fn a_second_holder_is_refused_until_the_first_lets_go() {
        let dir = std::env::temp_dir().join(format!("th-single-instance-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tinhat.lock");
        let first = lock_file_exclusive(&path).unwrap().expect("first instance takes the lock");
        assert!(lock_file_exclusive(&path).unwrap().is_none(), "a second instance must be refused");
        drop(first);
        let again = lock_file_exclusive(&path).unwrap();
        assert!(again.is_some(), "free again once the first instance is gone");
        drop(again);
        assert!(lock_file_exclusive(&dir.join("missing").join("tinhat.lock")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}


/// Block SIGTERM/SIGINT/SIGHUP in every thread and reap them on one dedicated thread, which
/// then runs the normal bounded leave-on-exit and terminates the process. `sigwait` keeps us
/// out of async-signal-handler context entirely (no allocation/locking restrictions), which
/// is what makes it safe to do real work — the same work the window-close path does.
#[cfg(unix)]
fn install_clean_exit_signals() {
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGHUP);
        // Block in THIS thread before any others spawn -> inherited by every thread, so the
        // default terminate-immediately disposition can never win the race.
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());

        std::thread::spawn(move || {
            let mut sig: i32 = 0;
            if libc::sigwait(&set, &mut sig) != 0 {
                return;
            }
            eprintln!("[exit] signal {sig} — leaving any active call before exit");
            bridge::runtime().block_on(async {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(15),
                    call_ctl::leave_on_exit(),
                )
                .await;
            });
            // 10.6 (REACH §2 / §3.4): a logout / kill is a quit too — the same presence goodbyes
            // as the window-close path (bounded at 3 s inside; a no-op when the engine never ran).
            bridge::runtime().block_on(presence::on_quit());
            eprintln!("[exit] clean exit complete");
            // _exit, not exit(): our cleanup is already done, and running libc atexit hooks +
            // static destructors while Qt's render thread and the GStreamer bus threads are
            // still live segfaults on the way out (ugly, and it makes a clean shutdown look
            // like a crash in the journal). The kernel reclaims everything anyway.
            libc::_exit(0);
        });
    }
}

/// Windows: no sigwait analog wired yet. The real thing (SetConsoleCtrlHandler +
/// WM_QUERYENDSESSION driving the same bounded leave-on-exit) lands with the
/// platform:: module — until then a killed Windows client can strand a phantom
/// call membership for peers (TEST_PLAN: "kill mid-call leaves no ghost call").
#[cfg(windows)]
fn install_clean_exit_signals() {
    // Window close / logoff run the bounded leave-on-exit via Qt's quit path
    // (verified: Alt+F4 mid-call -> "[call] left call on exit"). Linux has no
    // SIGTERM handler either, so there is no extra signal surface to mirror; a
    // hard kill relies on the SFU membership timeout on BOTH platforms.
    eprintln!("[main] clean exit rides the Qt quit path (window close / logoff); hard kill = SFU timeout");
}

/// Windows twin of the flock guard (PLATFORM_AUDIT.md §8, the CreateFileW variant):
/// open a per-user lock file with `share_mode(0)` (= `dwShareMode = 0`), so a SECOND
/// open fails with ERROR_SHARING_VIOLATION while the first instance lives. The OS
/// releases the handle when the process dies (even on crash) — no stale-lock risk.
/// Same purpose as on Linux: a 2nd instance would mint the identical `user:device`
/// LiveKit identity and wedge the first one's call rejoin.
#[cfg(windows)]
fn acquire_single_instance_lock() -> Option<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    if std::env::var_os("TINHAT_NO_SINGLE_INSTANCE").is_some() {
        return None;
    }
    const ERROR_SHARING_VIOLATION: i32 = 32;
    // `%LOCALAPPDATA%\TinHatApp\run` (the installer checks this exact file). It is the
    // same data root as every other path; this used to be its own LOCALAPPDATA read, which
    // became a folder relative to the launch folder when the variable was set but empty.
    let dir = platform::app_data_dir().join("run");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("tinhat.lock");
    // Sharing checks are bidirectional: a TRANSIENT third-party read handle (AV scan,
    // indexer — classically triggered by the previous instance closing this very file)
    // also fails a share_mode(0) open with error 32. Retry briefly before concluding
    // "already running": a real live instance holds its handle for its lifetime, so
    // genuine duplicates still fail every attempt; only scanner blips get rescued.
    // (flock on Linux has no such false positive — Windows-only concern.)
    let mut attempt = 0;
    loop {
        match std::fs::OpenOptions::new().create(true).write(true).share_mode(0).open(&path) {
            Ok(file) => {
                eprintln!("[main] single-instance lock acquired");
                return Some(file);
            }
            Err(e) if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION) && attempt < 4 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            Err(e) if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION) => {
                eprintln!("[main] another Tin Hat instance is already running — exiting this one.");
                eprintln!("[main] (set TINHAT_NO_SINGLE_INSTANCE=1 to force a second instance.)");
                println!("Tin Hat is already running.");
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("[main] single-instance lock unavailable ({e}); continuing without guard");
                return None;
            }
        }
    }
}


fn main() {
    boot::mark_start(); // anchors the [boot] +N ms timings
    // selfhost: the server gates go over the actions table before anything reads it (server_offer.rs)
    server_offer::ensure_actions_overlay();
    // The per-user directories are fixed FIRST, before anything below reads or writes a
    // file. Windows has no HOME, so every `$XDG_CONFIG_HOME, else $HOME/.config` path used to
    // be relative to the launch folder until init_bundle_env() — and the safe-mode decision
    // runs before that: its "starting" marker never got cleared, so every second start from
    // the same place ran in safe mode. Exports XDG_CONFIG_HOME/XDG_CACHE_HOME into the data
    // root on Windows; nothing on Linux/Flatpak.
    platform::init_dirs();
    // Windows "Apps & features" entry point for a zip-installed copy (0.8.16): the
    // uninstaller IS this exe. Handled before the single-instance lock — the running
    // app must not reject its own uninstaller, and nothing below should start.
    #[cfg(windows)]
    if std::env::args().any(|a| a == "--uninstall") {
        std::process::exit(platform::uninstall_portable_install());
    }
    // TINHAT_INSTALL_TEST=1 (Windows, headless): run the portable -> installed copy step
    // and exit — the release gate for "Install to your apps" without driving the GUI
    // (same idea as TINHAT_UPDATE_TEST). Exit 0 = installed, 1 = failed.
    #[cfg(windows)]
    if std::env::var_os("TINHAT_INSTALL_TEST").is_some() {
        match platform::install_from_portable() {
            Ok(p) => {
                eprintln!("[install-test] OK -> {}", p.display());
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("[install-test] FAIL: {e}");
                std::process::exit(1);
            }
        }
    }

    // Single-instance guard FIRST — BEFORE redirect_diagnostics_to_file() truncates the log.
    // A rejected 2nd launch (e.g. while a zombie/ghost still holds the lock) then exits here
    // WITHOUT wiping the existing instance's log — which is exactly the trace
    // we need to debug the zombie. Held for the whole process lifetime; released on exit/crash.
    // Bypass: TINHAT_NO_SINGLE_INSTANCE=1. Kept in INSTANCE_LOCK so it outlives app.exec(); a
    // Flatpak relaunch's child waits for it until this process is gone.
    *INSTANCE_LOCK.lock().unwrap() = acquire_single_instance_lock();

    // Safe mode (0.8.25): decided from the previous start's marker, exported into the
    // environment before Qt / WebEngine / GStreamer read it. See boot.rs. AFTER the
    // single-instance guard (0.8.28): a rejected second launch used to arm the marker
    // and the next real start ran in safe mode for no reason.
    boot::apply_safe_mode_env();

    // Platform early hook: Windows sweeps updater leftovers (backup\/staging\ — only
    // deletable now that the old process is gone) and wires the portable bundle's
    // GStreamer/QML env BEFORE gst::init/QGuiApplication. No-op on Linux.
    platform::early_startup();

    // Mirror ALL diagnostics (eprintln + tracing) to a file so we can debug a call
    // regardless of how the app was launched (terminal, .desktop, SDDM). Truncated
    // each run. stdout still prints a pointer to it.
    redirect_diagnostics_to_file();

    // Version banner, FIRST thing in every log: shipped logs carried no version at all,
    // so triage had to infer the build from log-format archaeology (one tester ran a 10-day-old
    // build for a week and nobody could tell from the uploads).
    eprintln!(
        "[main] Tin Hat {} · build {} ({}) · {}",
        update::UNIVERSAL_VERSION,
        env!("CARGO_PKG_VERSION"),
        option_env!("TINHAT_BUILD_HASH").unwrap_or("dev"),
        std::env::consts::OS
    );
    // Windows: the safe-mode decision above ran before stderr reached the log file (a shortcut
    // or Explorer start has no stderr at all), so its lines were lost. One summary line here:
    // marker path, what the previous start left, this start's mode.
    #[cfg(windows)]
    boot::report_start_marker();

    // TERM/INT/HUP: run the SAME clean-exit path as a window close, so a `kill`, a logout,
    // or a session teardown doesn't strand us in the call. Without this the app dies with its
    // MatrixRTC membership still published, and every peer's room keeps showing a phantom
    // "There's an active call in this room" until the 4h expiry (observed: three stale ghosts
    // left behind by killed instances). We block the signals process-wide and sigwait() for
    // them on a dedicated thread, so NOTHING async-signal-unsafe runs in a handler context —
    // the thread just does the normal bounded leave (LiveKit close + membership withdrawal,
    // which also NULLs the capture pipelines) and then exits.
    install_clean_exit_signals();

    // Log panics (with a backtrace) to the diagnostics log instead of letting them vanish.
    // A panic that unwinds out of a CXX-Qt `extern "RustQt"` invokable aborts the process; the
    // default hook's message can be swallowed by Qt's logging, leaving no trace. This turns a
    // silent invokable-abort into a readable breadcrumb. Installed right AFTER the stderr->file
    // redirect so it lands in the log. Does NOT change the panic strategy — we keep
    // `panic=unwind` (the tokio task-panic isolation + any future catch_unwind depend on it).
    std::panic::set_hook(Box::new(|info| {
        let bt = std::backtrace::Backtrace::force_capture();
        eprintln!("[panic] {info}\n[panic] backtrace:\n{bt}");
    }));

    // Qt6 routes QML qWarning/console.* to journald (not stderr) when stderr isn't a
    // TTY — which it isn't here (we dup2 it to a file). Force stderr logging so QML
    // errors/warnings land in the diagnostics log instead of vanishing. (QT_LOGGING_TO_CONSOLE
    // is deprecated in Qt6 → use QT_FORCE_STDERR_LOGGING + QT_ASSUME_STDERR_HAS_CONSOLE.)
    std::env::set_var("QT_FORCE_STDERR_LOGGING", "1");
    std::env::set_var("QT_ASSUME_STDERR_HAS_CONSOLE", "1");

    // Generate the Tin Hat KDE color schemes (OLED/Goose/Furry) so the whole app —
    // Kirigami chrome included — can be themed via KColorSchemeManager. Must be written
    // before the first scheme activation (QML Component.onCompleted).
    colorscheme::ensure_color_schemes();
    // Unpack the bundled Furry-theme assets (Blobfox + paw SVGs) to the runtime dir.
    assets::ensure_assets();

    // GstGL ↔ Qt context sharing: the qml6 video sink wraps Qt's own scenegraph GL
    // context for GStreamer, so gstgl must use the SAME platform/API as Qt. That depends
    // on the QPA platform — EGL/GLES2 on Wayland, GLX/desktop-GL on xcb (X11/XWayland) —
    // which isn't known until QGuiApplication is constructed. So the pin is applied
    // conditionally in `video::pin_gstgl_for_qpa()` right AFTER QGuiApplication::new()
    // (below), NOT here. Pinning EGL/GLES2 unconditionally broke the xcb/Intel path:
    // glupload couldn't find an API-compatible context → black tiles. gstgl reads these
    // vars lazily at GL-context creation (pipeline PLAYING, during a call), so setting
    // them just after QGuiApplication is in time.

    // Resolve the rustls CryptoProvider ambiguity (matrix-sdk pulls aws-lc-rs,
    // livekit pulls ring → rustls can't auto-pick → it panics on TLS setup, which
    // on a tokio worker thread silently wedged the LiveKit `wss://` connect). Pin
    // aws-lc-rs (what matrix-sdk already uses) as the process default.
    if rustls::crypto::aws_lc_rs::default_provider().install_default().is_err() {
        // Err == a process-default provider was already installed. Not fatal (ours matches
        // matrix-sdk's aws-lc-rs), but log it instead of silently swallowing — a surprise
        // provider here is exactly what silently wedged the LiveKit wss:// connect before.
        eprintln!("[main] rustls: a crypto provider was already installed; aws-lc-rs install skipped");
    }

    // Bridge the `log` crate (livekit/libwebrtc) into tracing, then RUST_LOG-driven
    // logging (also surfaces matrix-sdk's tracing).
    let _ = tracing_log::LogTracer::init();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();

    // Headless self-test of the self-update cycle (TEST_PLAN.md §2.8) against a TEST
    // manifest (point TINHAT_UPDATE_URL at a local server — NEVER the real channel):
    //   TINHAT_UPDATE_TEST=apply   check → download → sha256 → install → spawn verify phase
    //   TINHAT_UPDATE_TEST=verify  post-restart: assert swept dirs + promoted VERSION
    #[cfg(windows)]
    if let Ok(mode) = std::env::var("TINHAT_UPDATE_TEST") {
        let _g = bridge::runtime().enter();
        update::headless_update_test(&mode); // exits the process
    }
    // 10.0.2: the Flatpak portal update path, headless — TINHAT_UPDATE_TEST=portal inside the sandbox
    // (a session bus with flatpak-portal on it; TINHAT_PORTAL_WAIT=<s> caps the wait for its report).
    #[cfg(target_os = "linux")]
    if std::env::var("TINHAT_UPDATE_TEST").as_deref() == Ok("portal") {
        let _g = bridge::runtime().enter();
        flatpak::headless_portal_test(); // exits the process
    }
    // 10.1.1: the instance the headless test relaunched into — record the running commit and exit.
    #[cfg(target_os = "linux")]
    if std::env::var("TINHAT_UPDATE_TEST").as_deref() == Ok("portal-child") {
        flatpak::portal_child_marker(); // exits the process
    }

    // Headless two-device interactive (emoji/SAS) verification self-test:
    //   XDG_DATA_HOME=/tmp/a TINHAT_SASTEST=vouch  TINHAT_USER=… TINHAT_PASS=… ./tinhat
    //   XDG_DATA_HOME=/tmp/b TINHAT_SASTEST=newdev TINHAT_USER=… TINHAT_PASS=… ./tinhat
    #[cfg(target_os = "linux")]
    if let Ok(role) = std::env::var("TINHAT_SASTEST") {
        sas_test::run(&role); // exits the process
    }

    // Headless proof that mic capture survives AEC reference-probe loss (the
    // output-flip mic kill seen in a field log): TINHAT_AECMIC_TEST=1 ./tinhat — both
    // platforms since 0.8.25 (the webrtcdsp tier is the Linux default too).
    if std::env::var_os("TINHAT_AECMIC_TEST").is_some() {
        tinhat_streamer::call::headless_aecmic_test(); // exits the process
    }
    // 0.8.31: headless echo-cancellation bench — the PRODUCTION mic chain with the
    // reference branch, its output written raw (F32LE mono 48 k) for the bench to measure:
    //   TINHAT_AEC_ERLE_TEST=<secs> [TINHAT_AEC_ERLE_EC=0|1] [TINHAT_AEC_ERLE_OUT=<sink>] TINHAT_AEC_ERLE_FILE=<path> ./tinhat
    if std::env::var_os("TINHAT_AEC_ERLE_TEST").is_some() {
        tinhat_streamer::call::headless_aec_erle_test(); // exits the process
    }

    // Headless self-test of the LiveKit connect+publish path in THIS binary's
    // environment (libwebrtc linked with Qt/matrix/gst), bypassing Qt + the portal:
    //   TINHAT_LKTEST=1 LIVEKIT_URL=… LK_TOKEN=… LK_IDENTITY=… ./tinhat
    if std::env::var_os("TINHAT_LKTEST").is_some() {
        let _g = bridge::runtime().enter();
        let url = std::env::var("LIVEKIT_URL").expect("LIVEKIT_URL");
        let jwt = std::env::var("LK_TOKEN").expect("LK_TOKEN");
        let id = std::env::var("LK_IDENTITY").unwrap_or_default();
        bridge::runtime().block_on(async {
            if let Err(e) = tinhat_streamer::lk_publish::selftest(&url, &jwt, &id, 10).await {
                eprintln!("[selftest] ERROR: {e}");
            }
        });
        return;
    }

    // Headless self-test of the unified CALL session's audio publish path:
    //   TINHAT_CALLTEST=1 LIVEKIT_URL=… LK_TOKEN=… LK_IDENTITY=… ./tinhat
    if std::env::var_os("TINHAT_CALLTEST").is_some() {
        let _g = bridge::runtime().enter();
        let url = std::env::var("LIVEKIT_URL").expect("LIVEKIT_URL");
        let jwt = std::env::var("LK_TOKEN").expect("LK_TOKEN");
        let id = std::env::var("LK_IDENTITY").unwrap_or_default();
        bridge::runtime().block_on(async {
            if let Err(e) = tinhat_streamer::CallSession::selftest_audio(&url, &jwt, &id, 6).await {
                eprintln!("[call-selftest] ERROR: {e}");
            }
        });
        return;
    }

    // Cheap camera-enumeration sanity check (no LiveKit needed):
    //   TINHAT_LISTCAMS=1 ./tinhat
    if std::env::var_os("TINHAT_LISTCAMS").is_some() {
        let _g = bridge::runtime().enter();
        let cams = tinhat_streamer::call::list_cameras();
        eprintln!("[listcams] {} camera(s):", cams.len());
        for (path, label) in &cams {
            eprintln!("[listcams]   {path}  —  {label}");
        }
        return;
    }

    // Headless round-trip of the per-user call audio prefs JSON (#5): write -> reload ->
    // verdict, then clean up the sentinel. Proves persistence.  TINHAT_AUDIOPREFS_TEST=1
    if std::env::var_os("TINHAT_AUDIOPREFS_TEST").is_some() {
        audio_prefs::headless_verify();
        return;
    }

    // Which video encoder backends libwebrtc actually offers at runtime (proves the
    // NVENC factory compiled in AND the GPU/libcuda is detected): TINHAT_ENCPROBE=1
    if std::env::var_os("TINHAT_ENCPROBE").is_some() {
        let _g = bridge::runtime().enter();
        let backends = tinhat_streamer::encoder_backends();
        eprintln!("[encprobe] available encoder backends = {backends:?}");
        eprintln!(
            "[encprobe] NVENC (hardware H.264) = {}",
            if tinhat_streamer::nvenc_available() { "AVAILABLE ✓" } else { "NOT available ✗" }
        );
        return;
    }

    // Headless camera stride probe (no LiveKit): confirms whether the camera buffers
    // are row-padded (caps stride != actual stride).  TINHAT_CAMPROBE=1 [TINHAT_CAM_DEVICE=…]
    if std::env::var_os("TINHAT_CAMPROBE").is_some() {
        let _g = bridge::runtime().enter();
        let dev = std::env::var("TINHAT_CAM_DEVICE").ok();
        tinhat_streamer::camera::probe_camera(dev.as_deref(), 3);
        return;
    }

    // Headless self-test of the camera capture+publish path (no GUI/render):
    //   TINHAT_CAMTEST=1 LIVEKIT_URL=… LK_TOKEN=… LK_IDENTITY=… [TINHAT_CAM_DEVICE=/dev/video0] ./tinhat
    if std::env::var_os("TINHAT_CAMTEST").is_some() {
        let _g = bridge::runtime().enter();
        let url = std::env::var("LIVEKIT_URL").expect("LIVEKIT_URL");
        let jwt = std::env::var("LK_TOKEN").expect("LK_TOKEN");
        let id = std::env::var("LK_IDENTITY").unwrap_or_default();
        let dev = std::env::var("TINHAT_CAM_DEVICE").ok();
        bridge::runtime().block_on(async {
            if let Err(e) =
                tinhat_streamer::CallSession::selftest_camera(&url, &jwt, &id, dev, 8).await
            {
                eprintln!("[cam-selftest] ERROR: {e}");
            }
        });
        return;
    }

    // Headless NO-HUMAN E2EE loopback self-test. Restores the saved Matrix session,
    // mints TWO LiveKit identities from the one login (distinct device ids via
    // lk-jwt), then publishes a GCM-encrypted synthetic track and decodes it
    // in-process — proving the receive crypto/identity/decode path AND revealing the
    // exact lk-jwt identity format, with zero live peers. Runs a POSITIVE case (key
    // installed) and a NEGATIVE control (no key => must NOT decode).
    //   TINHAT_E2EE_LOOPBACK=1 ./tinhat
    if std::env::var_os("TINHAT_E2EE_LOOPBACK").is_some() {
        let _g = bridge::runtime().enter();
        bridge::runtime().block_on(async {
            let client = match bridge::restore_client().await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[loopback] restore failed (log in via the app first): {e}");
                    return;
                }
            };
            let svc = &rtc::focus_service_url(&client).await;
            let room = "tinhat-e2ee-loopback";
            let mint = |dev: &'static str| {
                let client = client.clone();
                async move { livekit::request_sfu_config_with_device(&client, svc, room, dev).await }
            };
            // POSITIVE: subscriber installs the publisher's key -> must decode.
            match (mint("loopPubA").await, mint("loopSubA").await) {
                (Ok(p), Ok(s)) => {
                    eprintln!("[loopback] SFU = {}", p.url);
                    let codec = std::env::var("TINHAT_E2EE_CODEC").unwrap_or_else(|_| "h264".into());
                    eprintln!("[loopback] publishing as {codec}");
                    match tinhat_streamer::call::e2ee_loopback_codec(&p.url, &p.jwt, &s.jwt, 12, true, &codec).await {
                        Ok(r) => print_loopback("POSITIVE", &r),
                        Err(e) => eprintln!("[loopback] POSITIVE run error: {e}"),
                    }
                }
                (a, b) => eprintln!("[loopback] POSITIVE token mint failed: {a:?} / {b:?}"),
            }
            // NEGATIVE control: no key installed -> must NOT decode (proves E2EE is on).
            match (mint("loopPubB").await, mint("loopSubB").await) {
                (Ok(p), Ok(s)) => {
                    match tinhat_streamer::call::e2ee_loopback(&p.url, &p.jwt, &s.jwt, 8, false).await {
                        Ok(r) => print_loopback("NEGATIVE", &r),
                        Err(e) => eprintln!("[loopback] NEGATIVE run error: {e}"),
                    }
                }
                _ => eprintln!("[loopback] NEGATIVE token mint failed"),
            }
        });
        return;
    }

    // Headless SHARE RECEIVE GATE: publish a screen-share-shaped track (single layer,
    // empty rid — the exact shape the r124 dynacast regression silently killed) into an
    // isolated test room on the live SFU, subscribe from a second participant, and require
    // decoded frames in BOTH 10s windows.   TINHAT_SHARETEST=1 ./tinhat
    // The ship-gate self-test family below (share/rejoin/audpub/vidpub/speaker/simtest)
    // assumes a Linux host (HANDOFF_WINDOWS.md §0) and hard-exits via libc::_exit, a
    // unix-only dep here — Linux-gated wholesale; Windows call testing goes through
    // windows/TEST_PLAN.md instead.
    #[cfg(target_os = "linux")]
    if std::env::var_os("TINHAT_SHARETEST").is_some() {
        let _g = bridge::runtime().enter();
        let ok = bridge::runtime().block_on(async {
            let client = match bridge::restore_client().await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[sharetest] restore failed (log in via the app first): {e}");
                    return false;
                }
            };
            let svc = &rtc::focus_service_url(&client).await;
            let room = "tinhat-sharetest";
            let mint = |dev: &'static str| {
                let client = client.clone();
                async move { livekit::request_sfu_config_with_device(&client, svc, room, dev).await }
            };
            match (mint("thShareA").await, mint("thShareB").await) {
                (Ok(p), Ok(s)) => {
                    match tinhat_streamer::call::share_loopback(&p.url, &p.jwt, &s.jwt).await {
                        Ok((a, b)) => {
                            // Healthy: ~300/window. Dynacast-kill signature: a<=2, b==0.
                            let pass = a > 50 && b > 50;
                            eprintln!(
                                "[sharetest] VERDICT: {} (first10s={a} second10s={b}, need >50 each)",
                                if pass { "PASS" } else { "FAIL" }
                            );
                            pass
                        }
                        Err(e) => {
                            eprintln!("[sharetest] error: {e}");
                            false
                        }
                    }
                }
                (a, b) => {
                    eprintln!("[sharetest] token mint failed: {:?} {:?}", a.err(), b.err());
                    false
                }
            }
        });
        // _exit: skip C++ static destructors (see FrameCryptor::~FrameCryptor).
        unsafe { libc::_exit(if ok { 0 } else { 1 }) };
    }

    // Headless REJOIN KEY GATE: prove the leave-and-rejoin flow that used to be the
    // join/leave audio coin flip — same identity reconnects with a NEW key at an
    // ADVANCED index, the subscriber's key install lands LATE (cryptor must recover),
    // and a stale same-index key must be REJECTED by the freshness registry.
    //   TINHAT_REJOIN_KEYTEST=1 ./tinhat
    #[cfg(target_os = "linux")]
    if std::env::var_os("TINHAT_REJOIN_KEYTEST").is_some() {
        let _g = bridge::runtime().enter();
        let ok = bridge::runtime().block_on(async {
            let client = match bridge::restore_client().await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[rejoin] restore failed (log in via the app first): {e}");
                    return false;
                }
            };
            let svc = &rtc::focus_service_url(&client).await;
            let room = "tinhat-rejoin-keytest";
            let mint = |dev: &'static str| {
                let client = client.clone();
                async move { livekit::request_sfu_config_with_device(&client, svc, room, dev).await }
            };
            // The two publisher JWTs use the SAME device name → same LiveKit identity,
            // which is exactly what a real app rejoin looks like to peers.
            match (mint("thRejoinSub").await, mint("thRejoinPub").await, mint("thRejoinPub").await) {
                (Ok(s), Ok(p1), Ok(p2)) => {
                    match tinhat_streamer::call::rejoin_keytest(&s.url, &s.jwt, &p1.jwt, &p2.jwt)
                        .await
                    {
                        Ok(r) => {
                            let pass = r.phase1_loud > 100
                                && r.phase2_loud > 100
                                && r.stale_rejected
                                && r.reinstall_ok;
                            eprintln!(
                                "[rejoin] VERDICT: {} (phase1_loud={} phase2_loud={} \
                                 stale_rejected={} reinstall_ok={}; need >100/>100/true/true)",
                                if pass { "PASS" } else { "FAIL" },
                                r.phase1_loud, r.phase2_loud, r.stale_rejected, r.reinstall_ok
                            );
                            pass
                        }
                        Err(e) => {
                            eprintln!("[rejoin] error: {e}");
                            false
                        }
                    }
                }
                (a, b, c) => {
                    eprintln!(
                        "[rejoin] token mint failed: {:?} {:?} {:?}",
                        a.err(), b.err(), c.err()
                    );
                    false
                }
            }
        });
        // _exit: skip C++ static destructors (see FrameCryptor::~FrameCryptor).
        unsafe { libc::_exit(if ok { 0 } else { 1 }) };
    }

    // DEV HELPER: TINHAT_AUDPUB=<matrix room id> — publish a synthetic talking/pausing
    // AUDIO track into that room for 2 minutes (headless), so a GUI instance joining it
    // exercises the REAL remote-audio playback path (gst pipeline + receive skew servo +
    // cushion prefill; watch for `[play-rt]`). No E2EE, like TINHAT_VIDPUB.
    #[cfg(target_os = "linux")]
    if let Ok(room) = std::env::var("TINHAT_AUDPUB") {
        if !room.is_empty() {
            let _g = bridge::runtime().enter();
            let ok = bridge::runtime().block_on(async {
                let client = match bridge::restore_client().await {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("[audpub] restore failed (log in via the app first): {e}");
                        return false;
                    }
                };
                match livekit::request_sfu_config_with_device(
                    &client,
                    &rtc::focus_service_url(&client).await,
                    &room,
                    "thAudPub",
                )
                .await
                {
                    Ok(cfg) => {
                        match tinhat_streamer::call::audio_publish_synthetic(
                            &cfg.url, &cfg.jwt, 120,
                        )
                        .await
                        {
                            Ok(()) => true,
                            Err(e) => {
                                eprintln!("[audpub] error: {e}");
                                false
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("[audpub] token mint failed: {e}");
                        false
                    }
                }
            });
            // _exit: skip C++ static destructors (see FrameCryptor::~FrameCryptor).
            unsafe { libc::_exit(if ok { 0 } else { 1 }) };
        }
    }

    // DEV HELPER: TINHAT_VIDPUB=<livekit room / matrix room id> — publish a synthetic
    // animated screen-share into that room for 3 minutes (headless), so the GUI shows a
    // real remote video tile without a second human. Used to exercise the PiP pop-out
    // (`--tinhat-popout`). No E2EE (enc_type=None tracks decode plainly in the app).
    #[cfg(target_os = "linux")]
    if let Ok(room) = std::env::var("TINHAT_VIDPUB") {
        if !room.is_empty() {
            let _g = bridge::runtime().enter();
            let ok = bridge::runtime().block_on(async {
                let client = match bridge::restore_client().await {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("[vidpub] restore failed (log in via the app first): {e}");
                        return false;
                    }
                };
                match livekit::request_sfu_config_with_device(
                    &client,
                    &rtc::focus_service_url(&client).await,
                    &room,
                    &std::env::var("TINHAT_VIDPUB_DEVICE").unwrap_or_else(|_| "thVidPub".to_string()),
                )
                .await
                {
                    Ok(cfg) => {
                        match tinhat_streamer::call::share_publish_synthetic(
                            &cfg.url, &cfg.jwt, 180,
                        )
                        .await
                        {
                            Ok(()) => true,
                            Err(e) => {
                                eprintln!("[vidpub] error: {e}");
                                false
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("[vidpub] token mint failed: {e}");
                        false
                    }
                }
            });
            // _exit: skip C++ static destructors (see FrameCryptor::~FrameCryptor).
            unsafe { libc::_exit(if ok { 0 } else { 1 }) };
        }
    }

    // ACTIVE-SPEAKERS GATE, child side: publish a steady tone as participant A from THIS
    // process, then exit. Forked by the TINHAT_SPEAKERTEST parent below — one process per
    // publisher (separate SDK stacks, like real users; see speakers_loopback's doc for
    // the history of why this shape was chosen).
    #[cfg(target_os = "linux")]
    if std::env::var_os("TINHAT_SPEAKERTEST_CHILD").is_some() {
        let url = std::env::var("TINHAT_SPK_URL").unwrap_or_default();
        let jwt = std::env::var("TINHAT_SPK_JWT").unwrap_or_default();
        let _g = bridge::runtime().enter();
        let ok = bridge::runtime().block_on(async {
            match tinhat_streamer::call::speakers_publish_steady(&url, &jwt, 45).await {
                Ok(()) => true,
                Err(e) => {
                    eprintln!("[speakertest-child] error: {e}");
                    false
                }
            }
        });
        // _exit: skip C++ static destructors (see FrameCryptor::~FrameCryptor).
        unsafe { libc::_exit(if ok { 0 } else { 1 }) };
    }

    // Headless ACTIVE-SPEAKERS GATE: two participants on the live SFU (a forked child
    // talks steadily as A; this process is B, whose level keeps stepping). B must keep A
    // in its active-speakers list with no gap >1s — the changed-only-delta wipe the r134
    // overlay shipped with cannot survive this.   TINHAT_SPEAKERTEST=1 ./tinhat
    #[cfg(target_os = "linux")]
    if std::env::var_os("TINHAT_SPEAKERTEST").is_some() {
        let _g = bridge::runtime().enter();
        let ok = bridge::runtime().block_on(async {
            let client = match bridge::restore_client().await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[speakertest] restore failed (log in via the app first): {e}");
                    return false;
                }
            };
            let svc = &rtc::focus_service_url(&client).await;
            let room = "tinhat-speakertest";
            let mint = |dev: &'static str| {
                let client = client.clone();
                async move { livekit::request_sfu_config_with_device(&client, svc, room, dev).await }
            };
            match (mint("thSpkA").await, mint("thSpkB").await) {
                (Ok(a), Ok(b)) => {
                    let exe = match std::env::current_exe() {
                        Ok(p) => p,
                        Err(e) => {
                            eprintln!("[speakertest] current_exe failed: {e}");
                            return false;
                        }
                    };
                    // Spawned from on_ready — i.e. only once B is connected AND listening.
                    // The steady child is mentioned in the SFU's changed-only deltas ONLY
                    // during its brief level convergence; spawning it any earlier can burn
                    // those mentions before B's listener exists -> false FAIL.
                    let mut child_slot: Option<std::process::Child> = None;
                    let r = tinhat_streamer::call::speakers_loopback(&b.url, &b.jwt, || {
                        child_slot = match std::process::Command::new(exe)
                            .env("TINHAT_SPEAKERTEST_CHILD", "1")
                            .env("TINHAT_NO_SINGLE_INSTANCE", "1")
                            .env("TINHAT_NO_LOG_REDIRECT", "1") // inherit OUR fd 2; don't truncate the log
                            .env_remove("TINHAT_SPEAKERTEST") // the child must not re-fork
                            .env("TINHAT_SPK_URL", &a.url)
                            .env("TINHAT_SPK_JWT", &a.jwt)
                            .spawn()
                        {
                            Ok(c) => Some(c),
                            Err(e) => {
                                // The run proceeds and FAILs as non-discriminating.
                                eprintln!("[speakertest] child spawn failed: {e}");
                                None
                            }
                        };
                    })
                    .await;
                    if let Some(mut child) = child_slot.take() {
                        let _ = child.kill(); // done observing; don't leave a toner running
                        let _ = child.wait();
                    }
                    match r {
                        Ok((active, total, max_gap_ms)) => {
                            // Healthy (merge semantics): A present in ~every sample, gaps 0.
                            // Replace-bug signature: presence blips of <=400ms between B's
                            // own deltas, gaps of many seconds — fails both bars. active==0
                            // also encodes a NON-DISCRIMINATING run (no delta churn).
                            let pass = active * 10 >= total * 9 && max_gap_ms <= 1000;
                            eprintln!(
                                "[speakertest] VERDICT: {} (A active {active}/{total}, max gap \
                                 {max_gap_ms}ms; need >=90% and <=1000ms)",
                                if pass { "PASS" } else { "FAIL" }
                            );
                            pass
                        }
                        Err(e) => {
                            eprintln!("[speakertest] error: {e}");
                            false
                        }
                    }
                }
                (a, b) => {
                    eprintln!("[speakertest] token mint failed: {:?} {:?}", a.err(), b.err());
                    false
                }
            }
        });
        // _exit: skip C++ static destructors (see FrameCryptor::~FrameCryptor).
        unsafe { libc::_exit(if ok { 0 } else { 1 }) };
    }

    // Headless NO-HUMAN SIMULCAST GATE (mirrors the loopback): publish a synthetic
    // camera track with the production simulcast options into an isolated test room on
    // the live SFU and verify (a) every H.264/NVENC rung really encodes (per-rid
    // outbound stats), (b) a subscriber's update_video_dimensions produces a real layer
    // switch on THIS SFU, (c) set_enabled(false/true) stops/restarts forwarding.
    //   TINHAT_CAM_SIMTEST=1 ./tinhat
    #[cfg(target_os = "linux")]
    if std::env::var_os("TINHAT_CAM_SIMTEST").is_some() {
        let _g = bridge::runtime().enter();
        let ok = bridge::runtime().block_on(async {
            let client = match bridge::restore_client().await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[simtest] restore failed (log in via the app first): {e}");
                    return false;
                }
            };
            let svc = &rtc::focus_service_url(&client).await;
            let room = "tinhat-simulcast-test";
            let mint = |dev: &'static str| {
                let client = client.clone();
                async move { livekit::request_sfu_config_with_device(&client, svc, room, dev).await }
            };
            match (mint("thSimPub").await, mint("thSimSub").await) {
                (Ok(p), Ok(s)) => {
                    eprintln!("[simtest] SFU = {}", p.url);
                    match tinhat_streamer::call::simulcast_loopback(&p.url, &p.jwt, &s.jwt).await {
                        Ok(reports) => {
                            let mut pass = true;
                            for r in &reports {
                                // 3 rungs at >=960px input; NVENC on every encoding rung.
                                let rungs_ok = r.rungs_encoding >= 3 && r.nvenc_rungs == r.rungs_encoding;
                                eprintln!(
                                    "[simtest] {}: rungs_encoding={} nvenc_rungs={} sub_frames={} => {}",
                                    r.phase, r.rungs_encoding, r.nvenc_rungs, r.sub_frames,
                                    if rungs_ok { "RUNGS OK" } else { "RUNGS FAIL" }
                                );
                                pass &= rungs_ok;
                                // The gate exists to catch RECEIVE-side regressions (it caught the
                                // NVDEC SEGV) — a subscriber that decodes zero frames, or that never
                                // ran the layer-switch probes (dims_before stays 0x0), must FAIL,
                                // not silently skip. Without this the gate PASSed on a dead recv path.
                                if r.sub_frames == 0 {
                                    eprintln!("[simtest] {}: subscriber decoded 0 frames => RECV FAIL", r.phase);
                                    pass = false;
                                }
                                if r.levers_expected && r.dims_before == (0, 0) {
                                    eprintln!("[simtest] {}: layer probes never ran (no subscription dims) => PROBE FAIL", r.phase);
                                    pass = false;
                                }
                                if r.dims_before != (0, 0) {
                                    // The invariant is "after asking for 320x180 we ARE on a low
                                    // rung" — not "the size strictly decreased". The SFU can
                                    // legitimately start us on the low rung already (BWE ramp-up),
                                    // in which case before==after==320x180 and a strict-decrease
                                    // check flakes FAIL on a perfectly healthy stream.
                                    let switched = r.dims_after.0 <= 640;
                                    eprintln!(
                                        "[simtest] {}: layer switch {}x{} -> {}x{} => {}; pause={} resume={}",
                                        r.phase, r.dims_before.0, r.dims_before.1, r.dims_after.0,
                                        r.dims_after.1, if switched { "OK" } else { "FAIL" },
                                        r.pause_worked, r.resume_worked
                                    );
                                    pass &= switched && r.pause_worked && r.resume_worked;
                                }
                            }
                            eprintln!("[simtest] VERDICT: {}", if pass { "PASS" } else { "FAIL" });
                            pass
                        }
                        Err(e) => {
                            eprintln!("[simtest] run error: {e}");
                            false
                        }
                    }
                }
                (a, b) => {
                    eprintln!("[simtest] token mint failed: {a:?} / {b:?}");
                    false
                }
            }
        });
        // _exit: skip C++ static destructors (see FrameCryptor::~FrameCryptor).
        unsafe { libc::_exit(if ok { 0 } else { 1 }) };
    }

    // Headless verification (no GUI): dump the peer keys the ROOM transport exposes
    // for a room, to confirm they parse under the live peer's identity.
    //   TINHAT_DUMP_ROOMKEYS='!room:server' ./tinhat
    if let Ok(room) = std::env::var("TINHAT_DUMP_ROOMKEYS") {
        let _g = bridge::runtime().enter();
        bridge::runtime().block_on(async {
            match bridge::restore_client().await {
                Ok(c) => {
                    session::set_client(c.clone());
                    for _ in 0..2 {
                        let _ = c.sync_once(crate::bridge::sync_settings()
                            .timeout(std::time::Duration::from_secs(10))).await;
                    }
                    call_ctl::dump_room_keys(&room).await;
                }
                Err(e) => eprintln!("[dumpkeys] restore failed: {e}"),
            }
        });
        return;
    }

    // Memories recorder service (0.8.7): headless, runs forever, joins calls whose
    // room carries dev.tinhat.record and captures per-track files. Runs on the recorder service host only.
    //   TINHAT_VOD_SERVICE=1 TINHAT_VOD_DIR=/vods ./tinhat
    if std::env::var_os("TINHAT_VOD_SERVICE").is_some() {
        let _g = bridge::runtime().enter();
        bridge::runtime().block_on(vodsvc::run());
        return;
    }

    // Headless verification (no GUI/render): auto-join a room's call, install peer
    // keys (room transport), and report decrypt state (recv4/recv5) for ~40s.
    //   TINHAT_AUTOJOIN='!room:server' ./tinhat
    if let Ok(room) = std::env::var("TINHAT_AUTOJOIN") {
        let _g = bridge::runtime().enter();
        bridge::runtime().block_on(async {
            match bridge::restore_client().await {
                Ok(c) => {
                    session::set_client(c.clone());
                    // Refresh room state (memberships/focus) before joining.
                    for _ in 0..3 {
                        let _ = c.sync_once(crate::bridge::sync_settings()
                            .timeout(std::time::Duration::from_secs(10))).await;
                    }
                    call_ctl::headless_join(&room, 40).await;
                }
                Err(e) => eprintln!("[autojoin] restore failed: {e}"),
            }
        });
        return;
    }

    // Headless verify of the per-room activity data layer (write -> read back), no GUI:
    //   TINHAT_ACTIVITY_ROOM='!room' TINHAT_ACTIVITY_SET='Helldivers 2' TINHAT_ACTIVITY_EMOJI='🎮'
    if let Ok(room) = std::env::var("TINHAT_ACTIVITY_ROOM") {
        let _g = bridge::runtime().enter();
        bridge::runtime().block_on(async {
            match bridge::restore_client().await {
                Ok(c) => {
                    session::set_client(c.clone());
                    for _ in 0..2 {
                        let _ = c.sync_once(crate::bridge::sync_settings()
                            .timeout(std::time::Duration::from_secs(10))).await;
                    }
                    activity::headless_verify(
                        &room,
                        std::env::var("TINHAT_ACTIVITY_SET").ok().as_deref(),
                        std::env::var("TINHAT_ACTIVITY_EMOJI").ok().as_deref(),
                    ).await;
                }
                Err(e) => eprintln!("[activity] restore failed: {e}"),
            }
        });
        return;
    }

    // Headless verify of the #7 AFK/BRB presence path (set away -> read back -> clear),
    // no GUI. Reports which transport worked (reaction vs custom-state fallback):
    //   TINHAT_AFK_TEST='!room:server' ./tinhat
    if let Ok(room) = std::env::var("TINHAT_AFK_TEST") {
        let _g = bridge::runtime().enter();
        bridge::runtime().block_on(async {
            match bridge::restore_client().await {
                Ok(c) => {
                    session::set_client(c.clone());
                    for _ in 0..2 {
                        let _ = c.sync_once(crate::bridge::sync_settings()
                            .timeout(std::time::Duration::from_secs(10))).await;
                    }
                    afk_ctl::headless_verify(&room).await;
                }
                Err(e) => eprintln!("[afk] restore failed: {e}"),
            }
        });
        return;
    }

    // Headless round-trip of the squad calendar (create → read+RSVP → vote → delete), no GUI:
    //   TINHAT_CALENDAR_TEST='!room:server' ./tinhat      (TINHAT_CALENDAR_KEEP=1 leaves the event)
    //   TINHAT_CALENDAR_CLEAN='!room:server' ./tinhat     (removes every caltest… event)
    if let Ok(room) = std::env::var("TINHAT_CALENDAR_TEST").or_else(|_| std::env::var("TINHAT_CALENDAR_CLEAN")) {
        let clean = std::env::var_os("TINHAT_CALENDAR_TEST").is_none();
        let _g = bridge::runtime().enter();
        bridge::runtime().block_on(async {
            match bridge::restore_client().await {
                Ok(c) => {
                    session::set_client(c.clone());
                    for _ in 0..2 {
                        let _ = c.sync_once(crate::bridge::sync_settings()
                            .timeout(std::time::Duration::from_secs(10))).await;
                    }
                    if clean { calendar::headless_clean(&room).await } else { calendar::headless_verify(&room).await }
                }
                Err(e) => eprintln!("[caltest] restore failed: {e}"),
            }
        });
        return;
    }

    // 10.5.1 headless proof of the Game Reel fixes (integer spin records + float-free Tune saves,
    // open roster, tonight's ad-hoc session) against the real homeserver — TEST rooms only:
    //   TINHAT_GAMES_TEST='!room:server' ./tinhat     (TINHAT_GAMES_KEEP=1 keeps the spin + prefs)
    //   TINHAT_GAMES_CLEAN='!room:server' ./tinhat    (empties my spin records + prefs there)
    if let Ok(room) = std::env::var("TINHAT_GAMES_TEST").or_else(|_| std::env::var("TINHAT_GAMES_CLEAN")) {
        let clean = std::env::var_os("TINHAT_GAMES_TEST").is_none();
        let _g = bridge::runtime().enter();
        bridge::runtime().block_on(async {
            match bridge::restore_client().await {
                Ok(c) => {
                    session::set_client(c.clone());
                    for _ in 0..2 {
                        let _ = c.sync_once(crate::bridge::sync_settings()
                            .timeout(std::time::Duration::from_secs(10))).await;
                    }
                    if clean { games::headless_clean(&room).await } else { games::headless_verify(&room).await }
                }
                Err(e) => eprintln!("[gamestest] restore failed: {e}"),
            }
        });
        return;
    }

    // Headless round-trip of the per-person status data layer (set/read/clear), no GUI:
    //   TINHAT_STATUS_TEST='!nondmroom:server' ./tinhat
    if let Ok(room) = std::env::var("TINHAT_STATUS_TEST") {
        let _g = bridge::runtime().enter();
        bridge::runtime().block_on(async {
            match bridge::restore_client().await {
                Ok(c) => {
                    session::set_client(c.clone());
                    for _ in 0..2 {
                        let _ = c.sync_once(crate::bridge::sync_settings()
                            .timeout(std::time::Duration::from_secs(10))).await;
                    }
                    status_ctl::headless_verify(&room).await;
                }
                Err(e) => eprintln!("[statustest] restore failed: {e}"),
            }
        });
        return;
    }

    // Headless parser dump (no client/session/GUI): print the exact embed JSON the QML
    // ThEmbedCard consumes for a message body, plus the onlyEmbed flag. Lets the inline
    // click-to-play card data be proven with zero render.
    //   TINHAT_DUMP_EMBEDS='check this https://youtu.be/dQw4w9WgXcQ and https://x.com/a.gif'
    if let Ok(body) = std::env::var("TINHAT_DUMP_EMBEDS") {
        let embeds = embeds::extract_embeds(&body);
        let only = embeds::body_is_only_embed(&body);
        eprintln!("[dumpembeds] onlyEmbed={only} count={}", embeds.len());
        eprintln!(
            "[dumpembeds] embeds={}",
            serde_json::to_string_pretty(&embeds).unwrap_or_default()
        );
        return;
    }

    // Headless round-trip of the soundboard data + search layer (no GUI): on a chosen
    // non-DM room — add a ROOM sound -> read-back -> tombstone -> read-back; set a PERSONAL
    // sound via account-data -> read back; and a proxy /soundboard/search round-trip.
    //   TINHAT_SOUNDBOARD_TEST='!nondmroom:server' [TINHAT_SOUNDBOARD_QUERY=airhorn] ./tinhat
    if let Ok(room) = std::env::var("TINHAT_SOUNDBOARD_TEST") {
        let _g = bridge::runtime().enter();
        bridge::runtime().block_on(async {
            match bridge::restore_client().await {
                Ok(c) => {
                    session::set_client(c.clone());
                    for _ in 0..2 {
                        let _ = c.sync_once(crate::bridge::sync_settings()
                            .timeout(std::time::Duration::from_secs(10))).await;
                    }
                    soundboard::headless_verify(&room).await;
                }
                Err(e) => eprintln!("[sbtest] restore failed: {e}"),
            }
        });
        return;
    }

    // Headless probe of the WATCH-PARTY data + sync-clock + search layer, no GUI:
    //   pure SYNC-DECISION assertions (seek/ride/no-thrash/applying-remote/manual-scrub),
    //   playback state round-trip, viewer heartbeat + expiry filter, a /watch/search trip.
    //   TINHAT_WATCHPARTY='!room:server' [TINHAT_WATCH_QUERY=lofi] ./tinhat
    if let Ok(room) = std::env::var("TINHAT_WATCHPARTY") {
        let _g = bridge::runtime().enter();
        bridge::runtime().block_on(async {
            match bridge::restore_client().await {
                Ok(c) => {
                    session::set_client(c.clone());
                    for _ in 0..2 {
                        let _ = c.sync_once(crate::bridge::sync_settings()
                            .timeout(std::time::Duration::from_secs(10))).await;
                    }
                    watchparty::headless_verify(&room).await;
                }
                Err(e) => eprintln!("[wptest] restore failed: {e}"),
            }
        });
        return;
    }

    // Headless round-trip of the GIF picker's proxy fetch (+ optional send), no GUI:
    //   TINHAT_GIF_TEST=1 [TINHAT_GIF_QUERY=cats] [TINHAT_GIF_SEND='!room:server'] ./tinhat
    if std::env::var_os("TINHAT_GIF_TEST").is_some() {
        let _g = bridge::runtime().enter();
        bridge::runtime().block_on(async { gif::headless_verify().await });
        return;
    }

    // Fire ONE sample desktop notification via org.freedesktop.Notifications (no GUI,
    // no second sender needed) — proves the notifier end-to-end against the live session
    // bus. Verifies the same Notify path used for new-highlight notifications.
    //   TINHAT_NOTIFY_TEST=1 ./tinhat
    if std::env::var_os("TINHAT_NOTIFY_TEST").is_some() {
        let _g = bridge::runtime().enter();
        bridge::runtime().block_on(async { notify::headless_test().await });
        return;
    }

    // Gate the creation flows + the call-permission template (QOL Phase 2):
    //   TINHAT_CREATE_TEST=1 TINHAT_USER=… TINHAT_PASS=… [TINHAT_USER2=…] ./tinhat
    if std::env::var_os("TINHAT_CREATE_TEST").is_some() {
        let _g = bridge::runtime().enter();
        let ok = bridge::runtime().block_on(async { bridge::headless_create_test().await });
        boot::exit_early(if ok { 0 } else { 1 });
    }

    // Gate that a room made while a space is selected lands IN that space, and that a
    // member of the space can join it with no invite:
    //   TINHAT_SPACEROOM_TEST=1 TINHAT_USER=… TINHAT_PASS=… TINHAT_USER2=… TINHAT_PASS2=… ./tinhat
    if std::env::var_os("TINHAT_SPACEROOM_TEST").is_some() {
        let _g = bridge::runtime().enter();
        let ok = bridge::runtime().block_on(async { bridge::headless_spaceroom_test().await });
        boot::exit_early(if ok { 0 } else { 1 });
    }

    // Gate that a received audio message resolves to a playable local file (QOL 1.6.1):
    //   TINHAT_AUDIO_TEST=1 TINHAT_USER=… TINHAT_PASS=… ./tinhat
    if std::env::var_os("TINHAT_AUDIO_TEST").is_some() {
        let _g = bridge::runtime().enter();
        let ok = bridge::runtime().block_on(async { room::headless_audio_test().await });
        boot::exit_early(if ok { 0 } else { 1 });
    }

    // Gate the push-to-talk decision table (QOL 3.4a): TINHAT_PTT_TEST=1 ./tinhat
    if std::env::var_os("TINHAT_PTT_TEST").is_some() {
        boot::exit_early(if call_ctl::headless_ptt_test() { 0 } else { 1 });
    }

    // Gate the join-sound decision table (QOL 1.5.2): TINHAT_JOINSOUND_TEST=1 ./tinhat
    if std::env::var_os("TINHAT_JOINSOUND_TEST").is_some() {
        boot::exit_early(if joinsound::headless_test() { 0 } else { 1 });
    }

    // Gate that profile fields (bio/accent) are PUBLIC, not private account data (QOL 1.5.1):
    //   TINHAT_PROFILE_TEST=1 TINHAT_USER=… TINHAT_PASS=… [TINHAT_USER2=… TINHAT_PASS2=…] ./tinhat
    if std::env::var_os("TINHAT_PROFILE_TEST").is_some() {
        let _g = bridge::runtime().enter();
        let ok = bridge::runtime().block_on(async { settings::headless_profile_test().await });
        boot::exit_early(if ok { 0 } else { 1 });
    }

    // Gate that a participant's real profile picture reaches the call tiles (QOL 1.5.0):
    //   TINHAT_CALLAVATAR=1 TINHAT_USER=… TINHAT_PASS=… ./tinhat
    if std::env::var_os("TINHAT_CALLAVATAR").is_some() {
        let _g = bridge::runtime().enter();
        let ok = bridge::runtime().block_on(async { call_ctl::headless_avatar_test().await });
        boot::exit_early(if ok { 0 } else { 1 });
    }

    // Gate the notification DECISION TABLE (which rooms toast, which stay silent) —
    // the logic a window-appeared smoke test can never reach.
    //   TINHAT_NOTIFY_RULES=1 [TINHAT_USER=… TINHAT_PASS=…] ./tinhat
    if std::env::var_os("TINHAT_NOTIFY_RULES").is_some() {
        let _g = bridge::runtime().enter();
        let ok = bridge::runtime().block_on(async { notify::headless_rules_test().await });
        boot::exit_early(if ok { 0 } else { 1 });
    }

    // Headless dump of the corner-dock nav data layer (space rail + active rooms +
    // global badge), no GUI:
    //   TINHAT_NAV_DUMP=1 [TINHAT_NAV_SPACE=people] ./tinhat
    if std::env::var_os("TINHAT_NAV_DUMP").is_some() {
        let _g = bridge::runtime().enter();
        bridge::runtime().block_on(async {
            match bridge::restore_client().await {
                Ok(c) => {
                    session::set_client(c.clone());
                    for _ in 0..2 {
                        let _ = c.sync_once(crate::bridge::sync_settings()
                            .timeout(std::time::Duration::from_secs(10))).await;
                    }
                    let space = std::env::var("TINHAT_NAV_SPACE").unwrap_or_else(|_| "home".into());
                    nav::headless_dump(&space).await;
                }
                Err(e) => eprintln!("[nav] restore failed: {e}"),
            }
        });
        return;
    }

    // Headless list of joined rooms (id + name), no GUI — find a room id for live testing:
    //   TINHAT_LIST_ROOMS=1 ./tinhat
    if std::env::var_os("TINHAT_LIST_ROOMS").is_some() {
        let _g = bridge::runtime().enter();
        bridge::runtime().block_on(async {
            match bridge::restore_client().await {
                Ok(c) => {
                    session::set_client(c.clone());
                    for _ in 0..2 {
                        let _ = c.sync_once(crate::bridge::sync_settings()
                            .timeout(std::time::Duration::from_secs(10))).await;
                    }
                    for room in c.joined_rooms() {
                        let name = room.name().unwrap_or_else(|| "(unnamed)".to_string());
                        eprintln!("[rooms] {} \"{}\"", room.room_id(), name);
                    }
                }
                Err(e) => eprintln!("[rooms] restore failed: {e}"),
            }
        });
        return;
    }

    // Install a tokio runtime EnterGuard on the Qt main thread for the WHOLE
    // program lifetime. matrix-sdk's sqlite store wraps each connection in
    // deadpool's `SyncWrapper`, whose `Drop` calls `tokio::task::spawn_blocking().unwrap()`.
    // When QObjects (and their `Client`) are destroyed during engine/app teardown on
    // the main thread — which has no runtime context — that unwrap panics
    // ("there is no reactor running") and the panic-in-destructor aborts the process.
    // The guard puts the runtime Handle in this thread's TLS so those destructors
    // succeed. MUST be a NAMED local (a bare `_` would drop at the semicolon) and
    // MUST outlive `app.exec()` AND the drop of `engine`/`app` below.
    let _rt_guard = bridge::runtime().enter();

    // Once-per-launch background sweep of the unbounded media/avatar disk caches
    // (LRU by mtime + TTL/size budget); detached, so it never blocks first paint.
    room::spawn_cache_sweep();

    // (single-instance guard acquired at the very top of main(), before the log redirect)

    // #9 PiP pop-out: install the streamer->PipVideoItem RGBA frame router (pure Rust
    // global — no Qt dependency, must simply exist before the first pop-out opens).
    pipvideo::install_pip_router();
    // (GStreamer init + the qml6glsink / mpvqt QML type registration moved BELOW the
    // startup window — 0.8.24 — so a first launch's registry scan is narrated, not blank.)

    // QtWebEngine (inline iframe embeds — YouTube/Twitch/Spotify/…). MUST run before
    // QGuiApplication (sets AA_ShareOpenGLContexts). Cheap until a WebEngineView is
    // actually created (lazily, on click) — no Chromium helper processes spawn here.
    // Safety valve: TINHAT_NO_WEBENGINE=1 skips init (iframe embeds then fall back to the
    // ↗ open-externally button) in case WebEngine ever destabilizes the scenegraph.
    if std::env::var_os("TINHAT_NO_WEBENGINE").is_none() {
        webengine::init();
    }

    let mut app = QGuiApplication::new();

    // Now that the QPA platform is resolved, pin GstGL to match Qt's GL context
    // (EGL/GLES2 on Wayland, GLX/desktop-GL on xcb). Must run before any call builds a
    // video pipeline. See the note next to the (removed) early GST_GL_* pin above.
    video::pin_gstgl_for_qpa();
    // 10.6 (REACH §2.4): Tin Hat's own input clock — the presence idle probe's last fallback.
    video::input_watch_install();

    // Qt's own per-user dirs (the QML disk cache, the WebEngine profile = cookies inside embeds) are
    // `<cache|data>/<organization>/<application>/`: name both, the same on every platform, instead of
    // Qt's "Unknown Organization" and the exe name. Set AFTER construction (so the ctor can't override)
    // and BEFORE the engine loads. The desktop file name is the app id (dev.tinhat.TinHat.desktop, the
    // Flatpak id): on Wayland it is the window's app_id, so the compositor finds the launcher, its icon
    // and the notification settings that belong to it.
    if let Some(mut app) = app.as_mut() {
        app.as_mut().set_organization_name(&QString::from("TinHat"));
        app.as_mut().set_application_name(&QString::from("tinhat"));
    }
    QGuiApplication::set_desktop_file_name(&QString::from("dev.tinhat.TinHat"));

    // TINHAT_QVS_TEST=1 (Windows): prototype 2 of HANDOFF §6 — prove the
    // appsink → QVideoFrame → QVideoSink → QML VideoOutput display tail under the
    // default D3D11 RHI. Opens its own test window + pipeline, prints a PASS/FAIL
    // verdict and exits. Runs INSTEAD of the app UI.
    #[cfg(windows)]
    if std::env::var_os("TINHAT_QVS_TEST").is_some() {
        let _ = tinhat_streamer::ensure_init(); // (normally done after the startup window)
        video::preinit();
        qvideo::run_selftest_pipeline();
        if let Some(app) = app.as_mut() {
            app.exec();
        }
        return;
    }

    // Windows theming shims (video_bridge.cpp): force the palette-driven Basic QQC2
    // style BEFORE the first Controls import (env-var route is unreliable across the
    // static-CRT exe ↔ Qt-DLL CRT boundary — verified live), and darken the DWM title
    // bars after the window exists (below).
    #[cfg(windows)]
    video::force_quick_style();

    // Icon theme. Windows: register the bundled breeze rcc (no system icon theme
    // exists there, so without this every Kirigami `icon.name` renders blank).
    // Linux: set breeze as the FALLBACK theme — on KDE that changes nothing, but on
    // GNOME/other desktops the breeze-specific names we use do not resolve and Qt
    // collapses an icon-only ToolButton to an invisible sliver (the room-list footer
    // buttons vanished entirely).
    iconpack::export_theme_env(); // 0.8.26: a pack's static theme wins over Breeze (Breeze stays the fallback)
    video::icon_theme();
    video::app_icon(); // 0.8.26: window/taskbar icon from the bundled app-icon PNG

    // The scene-graph RHI choice must land before the FIRST QQuickWindow — which is now
    // the startup window, not Main.qml's. Needs no GStreamer.
    video::set_graphics_api();

    let mut engine = QQmlApplicationEngine::new();

    // 0.8.24: the startup window goes up BEFORE the slow parts of a launch (GStreamer
    // registry scan, the Main.qml load, session restore) so a cold start never shows
    // nothing — people read "nothing" as broken. Skipped on the diagnostic root
    // overrides, which load their own QML instead of the app UI.
    #[cfg(windows)]
    let diagnostic_root = std::env::var_os("TINHAT_QML_OVERRIDE").is_some()
        || std::env::var_os("TINHAT_CAM_GLTEST").is_some()
        || std::env::var_os("TINHAT_BUTTONS_TEST").is_some();
    #[cfg(not(windows))]
    let diagnostic_root = std::env::var_os("TINHAT_CAM_GLTEST").is_some() || std::env::var_os("TINHAT_BUTTONS_TEST").is_some();
    if let (Some(engine), false) = (engine.as_mut(), diagnostic_root) {
        engine.load(&QUrl::from("qrc:/qt/qml/dev/tinhat/app/qml/StartupWindow.qml"));
        boot::pump(); // first paint before anything blocks
    }

    // Native remote-video tiles render via qml6glsink, which needs its QML type registered
    // before Main.qml loads; gst must be initialized first (preinit probe-creates the sink).
    // A first launch after install/update rescans the plugin registry here — seconds on
    // Windows — which is exactly what the startup window narrates.
    boot::set_stage("Starting the media engine", "", 20);
    boot::hold();
    let _ = tinhat_streamer::ensure_init();
    if !boot::safe_mode() {
        video::preinit(); // the qml6glsink GL probe is exactly what safe mode avoids
    }
    // Register the upstream mpvqt video item as the QML type `MpvVideo`
    // (dev.tinhat.mpv) for the watch-party player surface. mpvqt ships no QML plugin,
    // so we register its C++ type directly; must precede the Main.qml load below.
    video::register_mpv();
    boot::set_stage("Loading the interface", "", 40);
    boot::hold();

    if let Some(engine) = engine.as_mut() {
        // TINHAT_QML_OVERRIDE=<path.qml> (Windows-only, like the other port diagnostics):
        // load an arbitrary QML file in THIS binary (same CRT/link/plugins as production)
        // instead of the app UI. Added while isolating the Windows load-finalize crash;
        // cfg-gated so the Linux binary stays byte-identical to mainline (a stale env
        // var must never swap the Linux UI).
        // TINHAT_CAM_GLTEST: load the camera GL-upload self-test window instead of the app
        // (validates camera-on against Qt's real GL context — see cam_gltest.rs).
        #[cfg(windows)]
        let override_qml = std::env::var("TINHAT_QML_OVERRIDE").ok();
        #[cfg(not(windows))]
        let override_qml: Option<String> = None;
        let qml = if let Some(p) = override_qml {
            format!("file:///{}", p.replace('\\', "/"))
        } else if std::env::var_os("TINHAT_CAM_GLTEST").is_some() {
            "qrc:/qt/qml/dev/tinhat/app/qml/CamGlTest.qml".to_string()
        } else if std::env::var_os("TINHAT_HOST_TEST").is_some() {
            // islands C4: the dock host's offscreen test (HostTest.qml: 20 solve vectors through the real DockHost)
            "qrc:/qt/qml/dev/tinhat/app/qml/HostTest.qml".to_string()
        } else if std::env::var_os("TINHAT_BUTTONS_TEST").is_some() {
            // 11.0 buttons: the offscreen review captures (ButtonsTest.qml: rows, button islands, the Ctrl+K palette)
            "qrc:/qt/qml/dev/tinhat/app/qml/ButtonsTest.qml".to_string()
        } else if std::env::var_os("TINHAT_THEME_GALLERY").is_some() {
            // 11.0 step 5: the theme editor's / share cards' review captures (ThemeEditorGallery.qml, no sign-in; the
            // web's #/__gallery/theme-editor + theme-share, the same states)
            "qrc:/qt/qml/dev/tinhat/app/qml/ThemeEditorGallery.qml".to_string()
        } else {
            "qrc:/qt/qml/dev/tinhat/app/qml/Main.qml".to_string()
        };
        engine.load(&QUrl::from(qml.as_str()));
    }

    // Dark DWM title bars (main window exists now; future windows hook focus-changed).
    #[cfg(windows)]
    video::dark_titlebars();

    if let Some(app) = app.as_mut() {
        app.exec();
    }

    // Clean window-close: leave any active call so we don't orphan a ghost SFU participant
    // (which can wedge the next rejoin in "connecting…") AND so the camera/mic pipelines are
    // NULLed (releasing /dev/video0). `app.exec()` now returns because Main.qml's root window
    // calls Qt.quit() onClosing (cxx-qt-lib 0.8 can't set quitOnLastWindowClosed from Rust,
    // and secondary windows otherwise keep the loop alive — the zombie-broadcast bug).
    // leave_on_exit() is bounded internally, but wrap it once more so a pathological stall can
    // NEVER hang the quit. leave_on_exit()'s synchronous local NULLs run before its first
    // await, so the camera is freed even if this outer timeout fires.
    bridge::runtime().block_on(async {
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            call_ctl::leave_on_exit(),
        )
        .await;
    });
    // 10.6 (REACH §2 / §3.4): a closed Tin Hat is neither "Online" nor "at the computer" —
    // DELETE this device's presence entry and send the last visible:false (bounded at 3 s).
    bridge::runtime().block_on(presence::on_quit());

    // Positive proof of a clean quit. Its absence in a shipped log used to be ambiguous
    // (a Windows quit-after-hangup logged nothing at all) — with it, a log that just
    // STOPS is a kill/crash, full stop (see platform::install_crash_breadcrumb).
    eprintln!("[main] clean exit complete (Qt loop ended, call teardown done)");

    // `engine` then `app` drop here -> QObjects (LoginController + its Client + the
    // sqlite pool) drop here, now with a runtime context from `_rt_guard`, which
    // drops last. No abort.
}
