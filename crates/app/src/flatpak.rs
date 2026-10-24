//! 10.0 Flatpak integration. Inside the sandbox the app must not overwrite itself; updates go
//! through the Flatpak portal's UpdateMonitor: `Update()` installs the newest commit in the repo
//! (the desktop asks the user once whether the app may update itself), and Restart relaunches
//! into it through the portal. The version check itself still reads our manifest, so the banner
//! appears the moment a release is published.
//!
//! 10.0.2 registered ONE lifelong monitor at boot because flatpak-portal's own poll runs
//! on a 30-min timer armed at the first registration (DEFAULT_UPDATE_POLL_TIMEOUT_SEC), never on
//! demand — and, wrongly, parked the click until that poll had reported the new commit.
//!
//! 10.1.1: `Update()` does not depend on the poll at all. flatpak-portal's handle_update spawns
//! `flatpak-portal --update <installation> <ref>` — a fresh update transaction against the remote —
//! whenever it is called (proved on Flatpak 1.18.2 with the poll 3600 s away: installed in 0.1 s).
//! So a click calls `Update()` at once and narrates Flatpak's progress; "nothing to update" (our
//! feed ahead of the mirror by seconds) ends the attempt with a try-again line. Restart =
//! `flatpak-spawn --latest-version tinhat` (the portal's Spawn with LATEST_VERSION; no extra
//! permission; the portal owns the child, so it outlives us), then exit. 12.0: the single-instance
//! lock is shared by every instance of the app and stays ours until we are gone; the relaunched
//! child (TINHAT_RELAUNCH=update) waits for it (main.rs acquire_single_instance_lock).

/// What the controller hears back, off the Qt thread: progress lines while Flatpak runs, then
/// exactly one `Finished`. The callback must not touch this module's state.
pub enum Event {
    Status(String),
    Finished(Result<(), String>),
}
pub type Done = Box<dyn Fn(Event) + Send>;

#[cfg(target_os = "linux")]
mod imp {
    use super::{Done, Event};
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

    const RUNNING: &str = "Updating through Flatpak… (the system may ask you once)";
    const NOTHING: &str = "Flatpak's repo doesn't have the new version yet — try again in a minute";

    #[derive(Default)]
    pub struct State {
        pub monitor: Option<zbus::Proxy<'static>>,
        pub monitor_error: Option<String>,
        /// The portal's latest report said the repo has a commit we don't run.
        pub seen_update: bool,
        pub local_commit: String,
        pub remote_commit: String,
        pub reports: u32,
        /// A click arrived before the monitor finished registering (the first seconds after
        /// boot): applied the moment it exists.
        pub pending: bool,
        pub applying: bool,
        pub on_done: Option<Done>,
    }
    pub fn state() -> &'static Mutex<State> {
        static S: OnceLock<Mutex<State>> = OnceLock::new();
        S.get_or_init(|| Mutex::new(State::default()))
    }
    fn short(c: &str) -> &str {
        if c.len() > 10 { &c[..10] } else { c }
    }

    /// Hand an event to the pending click's callback (under the lock — the callbacks only queue
    /// onto the Qt thread or a channel). `Finished` consumes the callback.
    fn report(ev: Event) {
        let mut s = state().lock().unwrap();
        match ev {
            Event::Status(_) => {
                if let Some(cb) = &s.on_done {
                    cb(ev);
                }
            }
            Event::Finished(_) => {
                if let Some(cb) = s.on_done.take() {
                    cb(ev);
                }
            }
        }
    }

    /// Register the lifelong monitor (idempotent). Called at boot from the first update check.
    pub fn start_monitor() {
        static STARTED: OnceLock<()> = OnceLock::new();
        if STARTED.set(()).is_err() {
            return;
        }
        crate::bridge::runtime().spawn(async move {
            if let Err(e) = monitor_loop().await {
                eprintln!("[flatpak] portal update monitor unavailable: {e}");
                let fail_click = {
                    let mut s = state().lock().unwrap();
                    s.monitor_error = Some(e.clone());
                    s.monitor = None;
                    std::mem::take(&mut s.pending)
                };
                if fail_click {
                    report(Event::Finished(Err(e)));
                }
            }
        });
    }

    async fn monitor_loop() -> Result<(), String> {
        use futures_util::StreamExt;
        let conn = zbus::Connection::session().await.map_err(|e| format!("session bus: {e}"))?;
        let portal = zbus::Proxy::new(
            &conn,
            "org.freedesktop.portal.Flatpak",
            "/org/freedesktop/portal/Flatpak",
            "org.freedesktop.portal.Flatpak",
        )
        .await
        .map_err(|e| format!("portal: {e}"))?;
        let opts: HashMap<&str, Value<'_>> = HashMap::new();
        let handle: OwnedObjectPath = portal
            .call("CreateUpdateMonitor", &(opts,))
            .await
            .map_err(|e| format!("CreateUpdateMonitor: {e}"))?;
        let mon = zbus::Proxy::new(
            &conn,
            "org.freedesktop.portal.Flatpak",
            handle.clone(),
            "org.freedesktop.portal.Flatpak.UpdateMonitor",
        )
        .await
        .map_err(|e| format!("monitor: {e}"))?;
        let mut avail = mon.receive_signal("UpdateAvailable").await.map_err(|e| format!("UpdateAvailable: {e}"))?;
        eprintln!("[flatpak] portal update monitor registered at {}", handle.as_str());
        let apply_now = {
            let mut s = state().lock().unwrap();
            s.monitor = Some(mon);
            s.pending && !s.applying
        };
        if apply_now {
            spawn_apply();
        }
        while let Some(msg) = avail.next().await {
            let info: HashMap<String, OwnedValue> = match msg.body().deserialize() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let get = |k: &str| info.get(k).and_then(|v| String::try_from(v.clone()).ok()).unwrap_or_default();
            let (local, remote) = (get("local-commit"), get("remote-commit"));
            let newer = !remote.is_empty() && remote != local;
            eprintln!(
                "[flatpak] portal report: {} (installed {}, repo {})",
                if newer { "a newer commit is in the repo" } else { "up to date" },
                short(&local),
                short(&remote)
            );
            let apply = {
                let mut s = state().lock().unwrap();
                s.seen_update = newer;
                s.local_commit = local;
                s.remote_commit = remote;
                s.reports += 1;
                newer && s.pending && !s.applying
            };
            if apply {
                spawn_apply();
            }
        }
        Err("the portal closed the update monitor".into())
    }

    fn spawn_apply() {
        {
            let mut s = state().lock().unwrap();
            if s.applying {
                return;
            }
            s.applying = true;
            s.pending = false;
        }
        crate::bridge::runtime().spawn(async move {
            let r = apply().await;
            {
                let mut s = state().lock().unwrap();
                s.applying = false;
                match &r {
                    Ok(()) => {
                        s.seen_update = false;
                        eprintln!("[flatpak] update installed — Restart runs it");
                    }
                    Err(e) => eprintln!("[update] flatpak portal update failed: {e}"),
                }
            }
            report(Event::Finished(r));
        });
    }

    async fn apply() -> Result<(), String> {
        use futures_util::StreamExt;
        let mon = state()
            .lock()
            .unwrap()
            .monitor
            .clone()
            .ok_or_else(|| "the Flatpak portal is not available".to_string())?;
        let mut progress = mon.receive_signal("Progress").await.map_err(|e| format!("Progress: {e}"))?;
        let opts: HashMap<&str, Value<'_>> = HashMap::new();
        mon.call_method("Update", &("", opts)).await.map_err(|e| format!("Update: {e}"))?;
        // Progress status: 0 running, 1 empty, 2 done, 3 failed. A download can be long on a slow
        // line — give up only when Flatpak has said nothing for 10 minutes.
        let idle = std::time::Duration::from_secs(600);
        let mut last = u32::MAX;
        loop {
            match tokio::time::timeout(idle, progress.next()).await {
                Ok(Some(msg)) => {
                    let Ok(map) = msg.body().deserialize::<HashMap<String, OwnedValue>>() else { continue };
                    let status = map.get("status").and_then(|v| u32::try_from(v.clone()).ok()).unwrap_or(0);
                    let pct = map.get("progress").and_then(|v| u32::try_from(v.clone()).ok()).unwrap_or(0);
                    if status == 0 {
                        if pct != last {
                            last = pct;
                            eprintln!("[flatpak] update progress {pct}%");
                            report(Event::Status(format!("Updating through Flatpak… {pct}%")));
                        }
                        continue;
                    }
                    if status == 2 {
                        return Ok(());
                    }
                    if status == 1 {
                        return Err(NOTHING.to_string());
                    }
                    let err = map.get("error_message").and_then(|v| String::try_from(v.clone()).ok()).unwrap_or_default();
                    return Err(if err.is_empty() { format!("Flatpak reported status {status}") } else { err });
                }
                Ok(None) => return Err("the portal closed the update monitor".to_string()),
                Err(_) => return Err("Flatpak stopped reporting progress".to_string()),
            }
        }
    }

    /// The banner's "Update now" inside a Flatpak: Ok(the line to show while it runs) or Err(why not).
    /// Calls `Update()` at once — the portal's own poll is irrelevant (see the module doc).
    pub fn request_apply(on_done: Done) -> Result<String, String> {
        start_monitor();
        let mut s = state().lock().unwrap();
        if let Some(e) = &s.monitor_error {
            return Err(format!("the Flatpak portal is not available ({e})"));
        }
        if s.applying {
            return Ok(RUNNING.into());
        }
        s.on_done = Some(on_done);
        if s.monitor.is_some() {
            drop(s);
            spawn_apply();
            Ok(RUNNING.into())
        } else {
            s.pending = true;
            Ok("Updating through Flatpak… (connecting to the portal)".into())
        }
    }

    /// Relaunch into the newest installed commit through the portal: `flatpak-spawn
    /// --latest-version` starts a fresh sandbox on the new deployment (Spawn flag LATEST_VERSION;
    /// needs no extra finish-args) and the portal owns that child, so it outlives us. The caller
    /// exits right after, still holding the shared single-instance lock: the child (it gets
    /// TINHAT_RELAUNCH=update) waits until we are gone. Our arguments carry over.
    /// flatpak-spawn forwards NOTHING of our process environment — the child gets the instance's
    /// original launch environment — so anything meant for the child goes through `--env=`.
    pub fn relaunch_latest(extra_env: &[(&str, &str)]) -> Result<(), String> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut cmd = std::process::Command::new("flatpak-spawn");
        cmd.arg("--latest-version").arg("--env=TINHAT_RELAUNCH=update");
        for (k, v) in extra_env {
            cmd.arg(format!("--env={k}={v}"));
        }
        cmd.arg("--").arg("tinhat").args(&args);
        cmd.stdin(std::process::Stdio::null());
        let child = cmd.spawn().map_err(|e| format!("flatpak-spawn: {e}"))?;
        eprintln!("[flatpak] relaunching into the latest installed version (flatpak-spawn pid {})", child.id());
        Ok(())
    }

    /// TINHAT_UPDATE_TEST=portal — run INSIDE the Flatpak on a session bus that has flatpak-portal:
    /// register the monitor, ask to apply as the banner's button would, follow the install
    /// (TINHAT_PORTAL_WAIT s cap, default 120), VERDICT line + exit code. With TINHAT_PORTAL_RELAUNCH=1
    /// a PASS goes on to relaunch through the portal exactly as Restart does; the child runs
    /// `portal-child` (below) and records which commit it is, after this process is gone.
    pub fn headless_portal_test() -> ! {
        let wait: u64 = std::env::var("TINHAT_PORTAL_WAIT").ok().and_then(|v| v.parse().ok()).unwrap_or(120);
        let relaunch = std::env::var_os("TINHAT_PORTAL_RELAUNCH").is_some();
        let rc = crate::bridge::runtime().block_on(async move {
            if !crate::update::in_flatpak() {
                eprintln!("[portaltest] VERDICT: FAIL — not inside a Flatpak");
                return 1;
            }
            let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
            let cb: Done = Box::new(move |ev| match ev {
                Event::Status(line) => eprintln!("[portaltest] status: {line}"),
                Event::Finished(r) => {
                    let _ = tx.send(r);
                }
            });
            let t0 = std::time::Instant::now();
            match request_apply(cb) {
                Ok(line) => eprintln!("[portaltest] {line}"),
                Err(e) => {
                    eprintln!("[portaltest] VERDICT: FAIL — {e}");
                    return 1;
                }
            }
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                if let Ok(r) = rx.try_recv() {
                    return match r {
                        Ok(()) => {
                            eprintln!(
                                "[portaltest] VERDICT: PASS — the portal installed the newer commit {:.1}s after the click",
                                t0.elapsed().as_secs_f32()
                            );
                            0
                        }
                        Err(e) => {
                            eprintln!("[portaltest] VERDICT: FAIL — {e}");
                            1
                        }
                    };
                }
                if let Some(e) = state().lock().unwrap().monitor_error.clone() {
                    eprintln!("[portaltest] VERDICT: FAIL — monitor: {e}");
                    return 1;
                }
                if t0.elapsed().as_secs() > wait {
                    eprintln!("[portaltest] VERDICT: FAIL — the install did not finish in {wait}s");
                    return 1;
                }
            }
        });
        if rc == 0 && relaunch {
            // the single-instance lock stays ours: the child waits for it until we have exited
            match relaunch_latest(&[("TINHAT_UPDATE_TEST", "portal-child")]) {
                Ok(()) => eprintln!("[portaltest] relaunch requested — exiting so the child proves it outlives us"),
                Err(e) => {
                    eprintln!("[portaltest] VERDICT: FAIL — relaunch: {e}");
                    std::process::exit(1);
                }
            }
        }
        std::process::exit(rc);
    }

    /// TINHAT_UPDATE_TEST=portal-child — the relaunched instance in the headless test: record which
    /// commit runs (from /.flatpak-info) in $XDG_DATA_HOME/portal-child.txt and exit.
    pub fn portal_child_marker() -> ! {
        // Write only after the parent is certainly gone: the marker then proves the child outlived it.
        std::thread::sleep(std::time::Duration::from_secs(2));
        let info = std::fs::read_to_string("/.flatpak-info").unwrap_or_default();
        let commit = info.lines().find_map(|l| l.strip_prefix("app-commit=")).unwrap_or("").to_string();
        let dir = std::env::var_os("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        let line = format!("app-commit={commit} pid={} ppid={}\n", std::process::id(), unsafe { libc::getppid() });
        if let Err(e) = std::fs::write(dir.join("portal-child.txt"), &line) {
            eprintln!("[portaltest] child: could not write the marker: {e}");
            std::process::exit(1);
        }
        eprint!("[portaltest] child: {line}");
        std::process::exit(0);
    }
}

#[cfg(target_os = "linux")]
pub use imp::{headless_portal_test, portal_child_marker, relaunch_latest, request_apply, start_monitor};

#[cfg(not(target_os = "linux"))]
pub fn start_monitor() {}
#[cfg(not(target_os = "linux"))]
pub fn request_apply(_on_done: Done) -> Result<String, String> {
    Err("not a Flatpak".into())
}
#[cfg(not(target_os = "linux"))]
pub fn relaunch_latest(_extra_env: &[(&str, &str)]) -> Result<(), String> {
    Err("not a Flatpak".into())
}
