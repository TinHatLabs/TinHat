//! Opt-out debug-log sharing and in-app problem reports — both go to the user's OWN server (its owner reads them on
//! the dashboard's Reports card), never anywhere else (selfhost, 2026-10-21; before: one central report host).
//!
//! * `POST <api>/reports` (JSON: type, title, body, platform, version, reporter; bearer = the Matrix token) files a
//!   report and answers `{ok, id}`; `POST <api>/logs/upload?name=<file>` (the raw, scrubbed log; bearer) keeps a
//!   log beside it. `<api>` is the server's announced API base (proxy.rs). Signed out there is nothing to send to.
//! * Only while the server offers `reports` (server_offer.rs): off, the passive upload stays home and the UI hides
//!   the buttons.
//!
//! Consent contract (shown in a first-run popup, toggle in Settings → Privacy):
//!   * default ON after the popup informs the user; one click turns it off; the
//!     choice persists at $XDG_CONFIG_HOME/tinhat/logship ("1"/"0"; absent = not
//!     yet asked → QML shows the popup).
//!   * scrubbed before anything leaves the machine: public IPs (v4/v6) → "[ip]",
//!     tokens/JWTs/key material via diagnostics::redact. Usernames stay (that's
//!     the whole point — knowing WHO hit the bug); room ids stay (server-side
//!     debugging needs them; the rooms are all on the user's own homeserver).
//!   * Both platforms since 0.8.x (the "Windows-only" note here was stale for months);
//!     0.8.29: Linux logs moved from /tmp to the per-user state dir and every
//!     attached section is stamped with its provenance.
//!
//! Unlike diagnostics.rs (whose header PROMISES no network code — respected, hence
//! this separate module), this module's one job is the upload.

use std::path::PathBuf;

/// The file THIS session's diagnostics are actually being written to.
///
/// The two platforms route stderr differently — main.rs dup2s it to /tmp on unix,
/// while platform::redirect_stderr_to_log sends it to %LOCALAPPDATA%\TinHatApp\logs
/// on Windows. Reading the Windows path unconditionally is why Linux bug reports
/// arrived with an empty tail while Windows ones were complete: the read just
/// failed and unwrap_or_default() turned "no such file" into "no log", silently.
fn live_log_path() -> Option<PathBuf> {
    // 0.8.29: what fd 2 ACTUALLY points at this run (or None) — never a constant.
    crate::platform::live_log_path()
}

/// The previous session's rotated log — rotation happens at startup on both platforms
/// (Windows: platform::early_startup; Linux: redirect_diagnostics_to_file).
fn old_log_path() -> PathBuf {
    crate::platform::old_log_path()
}

/// 0.8.29: say WHICH build wrote an attached log section and whether it can be
/// this session's at all. One field report was stamped 0.8.28 and carried a 32-line 0.8.28 launch
/// that ended with its own `[main] clean exit complete` — a line printed only after the
/// Qt loop returns, so no running process can read it as its own last bytes — followed
/// by 800 KB of 0.8.27. Nobody could tell from the report. Now every section says
/// `build X (hash), <file>, modified N s ago`, and a dead-process tail or a missing
/// live log is called out instead of silently attaching a stranger's file.
fn provenance(text: &str, path: Option<&std::path::Path>) -> String {
    let Some(path) = path else {
        return "NOT AVAILABLE: stderr was not redirected to a file this run".to_string();
    };
    let build = regex::Regex::new(r"· build (\S+ \([0-9a-f]+\))")
        .ok()
        .and_then(|re| re.captures(text).map(|c| c[1].to_string()))
        .unwrap_or_else(|| "unknown build (no banner in this tail)".to_string());
    let age = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|d| format!("modified {} s ago", d.as_secs()))
        .unwrap_or_else(|| "mtime unknown".to_string());
    let dead = text
        .trim_end()
        .lines()
        .rev()
        .take(3)
        .any(|l| l.contains("[main] clean exit complete"));
    format!(
        "build {build}, {}, {age}{}",
        path.display(),
        if dead {
            "; WARNING: ends with a clean-exit line — written by an earlier, already-finished \
             process, NOT this session"
        } else {
            ""
        }
    )
}

fn consent_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    base.join("tinhat/logship")
}

/// -1 = never asked (QML shows the consent popup), 0 = off, 1 = on.
pub fn consent_state() -> i32 {
    match std::fs::read_to_string(consent_path()) {
        Ok(s) if s.trim() == "1" => 1,
        Ok(_) => 0,
        Err(_) => -1,
    }
}

pub fn set_consent(on: bool) {
    let p = consent_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&p, if on { "1" } else { "0" });
    eprintln!("[logship] consent set: {}", if on { "on" } else { "off" });
}

// ---- whose log is it? (Astra review 2026-10-21: logs must not cross accounts) -----------------------------------
// Logs go to the signed-in account's own server. A log that also holds another account's session (someone signed out
// and someone else signed in on this device) must not reach the second account's server owner. The device file
// `<config>/tinhat/logship-owner` names the account whose session the CURRENT log belongs to ("" = none / mixed);
// at start-up it describes the log that was just rotated to `.old.log` (read once, before anyone signs in).

fn owner_path() -> PathBuf {
    consent_path().with_file_name("logship-owner")
}

/// Somebody signed out during this run: the live log holds more than one account from here on.
static SIGNED_OUT_THIS_RUN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The account the previous session's log (`.old.log`) belongs to ("" = none or more than one).
fn prev_owner() -> &'static str {
    static P: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    P.get_or_init(|| std::fs::read_to_string(owner_path()).map(|s| s.trim().to_string()).unwrap_or_default())
}

/// The previous session's log may go to `user`'s server: it was that account's alone.
pub(crate) fn prev_log_is(user: &str, prev_owner: &str) -> bool {
    !user.is_empty() && prev_owner == user
}

/// A sign-in (fresh or restored): this run's log is `user`'s — unless another account already signed out in this run.
pub fn note_signed_in(user: &str) {
    let _ = prev_owner(); // read the previous session's owner before this run's is written
    let p = owner_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let who = if SIGNED_OUT_THIS_RUN.load(std::sync::atomic::Ordering::SeqCst) { "" } else { user };
    let _ = std::fs::write(&p, who);
}

/// A sign-out: from here on this run's log is nobody's alone.
pub fn note_signed_out() {
    let _ = prev_owner();
    SIGNED_OUT_THIS_RUN.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = std::fs::write(owner_path(), "");
}

/// Strip public IPs on top of diagnostics' token/secret redaction. v4 loosely, v6 by
/// hextet runs; loopback/RFC1918 kept (useful, not personal — but the regex doesn't
/// bother distinguishing: every ip-shaped token becomes "[ip]", simpler to reason
/// about and nothing in the logs needs a literal address to stay useful).
fn scrub(input: &str) -> String {
    let redacted = crate::diagnostics::redact(input);
    let v4 = regex::Regex::new(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b").unwrap();
    let v6 = regex::Regex::new(r"\b(?:[0-9a-fA-F]{1,4}:){2,7}[0-9a-fA-F]{0,4}\b").unwrap();
    let s = v4.replace_all(&redacted, "[ip]");
    v6.replace_all(&s, "[ip]").into_owned()
}

/// The localpart of a user id as a file-name part (letters, digits, `-_.`); "unknown" when nothing is left.
fn who_of(user: &str) -> String {
    let who: String = user
        .trim_start_matches('@')
        .split(':')
        .next()
        .unwrap_or("unknown")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
        .collect();
    if who.is_empty() {
        "unknown".to_string()
    } else {
        who
    }
}

/// The server's API base and this session's token: where a report or a log can go. None signed out (or before the
/// server is known) — then nothing is sent anywhere.
fn destination() -> Option<(String, String)> {
    let base = crate::proxy::base();
    let token = crate::session::client().and_then(|c| c.access_token())?;
    if base.is_empty() || token.is_empty() {
        return None;
    }
    Some((base, token))
}

/// `<api>/logs/upload?name=<file>`: the upload URL of a log named `file` (the server keeps the name beside the
/// sender's id and a time stamp).
pub fn log_upload_url(base: &str, file: &str) -> String {
    let name: String = file.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect();
    format!("{base}/logs/upload?name={name}")
}

/// File an in-app problem report (the room-list 🐞 button): the CLICK is the consent, so this ships even when the
/// passive logship toggle is off. Uploads the user's note plus the TAIL of the LIVE session log — no restart needed
/// to get evidence out (our own append handle was opened with share-read, so a second in-process read handle is
/// fine). Returns the server's report id so the UI can show the user something they can follow, or "" when the
/// report could not be filed (the log still ships when it can).
pub async fn ship_bug_report(user: &str, note: &str) -> Result<String, String> {
    const TAIL_BYTES: usize = 2 * 1024 * 1024;
    // the UI hides the button where the server takes no reports (and for guest accounts); this is the same rule
    if !crate::server_offer::feature_on_global(crate::server_offer::ServerFeature::Reports) {
        return Err("this server doesn't take problem reports".to_string());
    }
    let Some((base, token)) = destination() else {
        return Err("not signed in to a server — nothing to send the report to".to_string());
    };
    // another account used Tin Hat earlier in this run: its part of the live log stays here
    let mixed = SIGNED_OUT_THIS_RUN.load(std::sync::atomic::Ordering::SeqCst);
    let live = if mixed { None } else { live_log_path() };
    let raw = live
        .as_ref()
        .and_then(|p| std::fs::read(p).ok())
        .map(|b| {
            let start = b.len().saturating_sub(TAIL_BYTES);
            String::from_utf8_lossy(&b[start..]).into_owned()
        })
        .unwrap_or_default();
    let live_prov = if mixed {
        "NOT ATTACHED: another account used Tin Hat earlier in this run".to_string()
    } else {
        provenance(&raw, live.as_deref())
    };
    let who = who_of(user);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // The previous session's tail rides along too: "it broke, I restarted, then filed
    // a report" otherwise ships a fresh-session log with zero evidence in it.
    let old = old_log_path();
    let prev = if prev_log_is(user, prev_owner()) { std::fs::read(&old).ok() } else { None }
        .map(|b| {
            let start = b.len().saturating_sub(TAIL_BYTES / 2);
            String::from_utf8_lossy(&b[start..]).into_owned()
        })
        .unwrap_or_default();
    let prev_prov = if prev.trim().is_empty() {
        "none".to_string()
    } else {
        provenance(&prev, Some(old.as_path()))
    };
    let prev_section = if prev.trim().is_empty() {
        String::new()
    } else {
        format!(
            "\n\n=== previous session log tail ({} KB, scrubbed; {}) ===\n{}",
            prev.len() / 1024,
            prev_prov,
            scrub(&prev)
        )
    };
    eprintln!("[logship] bug report provenance: live = {live_prov}; previous = {prev_prov}");
    // The platform is stated explicitly rather than left for the receiver to guess: a receiver used to infer it
    // from path shapes in the log tail, which reads "unknown" whenever the tail is short or the report is filed
    // early in a session.
    let version = format!("{} ({})", crate::update::UNIVERSAL_VERSION, env!("CARGO_PKG_VERSION"));
    let body = format!(
        "=== BUG REPORT from {user} (app v{version}, {}) ===\n{}\n\n=== live session log tail ({} KB, scrubbed; {}) ===\n{}{}",
        std::env::consts::OS,
        note.trim(),
        raw.len() / 1024,
        live_prov,
        scrub(&raw),
        prev_section
    );
    let client = crate::proxy::http(None);

    // Step 1: file the report, so we can hand the user its id. Best-effort — if that fails we still ship the log
    // below (the owner sees it on the same card).
    let title = note
        .trim()
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .chars()
        .take(160)
        .collect::<String>();
    let title = if title.is_empty() {
        format!("[in-app] bug report from {user}")
    } else {
        format!("[in-app] {title}")
    };
    // Body is serialized by hand and the header set explicitly: matrix-sdk re-exports
    // reqwest without its "json" feature, so RequestBuilder::json does not exist here.
    let payload = serde_json::json!({
            "type": "bug",
            "title": title,
            "body": format!(
                "{}\n\n— submitted from the {} app ({version}); full session log attached.\nlog provenance: live = {}; previous = {}",
                note.trim(),
                std::env::consts::OS,
                live_prov,
                prev_prov,
            ),
            "platform": std::env::consts::OS,
            "version": version,
            "reporter": user,
    })
    .to_string();
    let report_url = format!("{base}/reports");
    let id = match client
        .post(&report_url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .body(payload)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r
            .text()
            .await
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v.get("id").and_then(|i| i.as_str().map(String::from)))
            .unwrap_or_default(),
        Ok(r) => {
            eprintln!("[logship] the server refused the report: HTTP {}", r.status());
            String::new()
        }
        Err(e) => {
            eprintln!("[logship] the server didn't take the report ({e}) — shipping the log alone");
            String::new()
        }
    };

    // Step 2: ship the log, named after the report when there is one, so the owner finds them together.
    let suffix = if id.is_empty() { String::new() } else { format!("-{id}") };
    let url = log_upload_url(&base, &format!("{who}-bugreport-{ts}{suffix}.log"));
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "text/plain; charset=utf-8")
        .header("X-TinHat-Version", env!("CARGO_PKG_VERSION"))
        .body(body)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("upload failed: {e}"))?;
    if resp.status().is_success() {
        eprintln!("[logship] bug report shipped ({url}) report={id:?}");
        Ok(id)
    } else if !id.is_empty() {
        // The report exists and is what the user was promised; a missing log tail is a
        // degraded report, not a failed one.
        eprintln!("[logship] report {id} filed but the log upload returned {}", resp.status());
        Ok(id)
    } else {
        Err(format!("server said HTTP {}", resp.status()))
    }
}

/// Ship the previous session's rotated log (complete by construction — rotation
/// happens at startup before anything writes). Fire-and-forget on the runtime;
/// failures log once and never bother the user. `user` tags the filename so the
/// owner's Reports card is browsable by person. Only while the server offers reports.
pub fn ship_previous_session_log(user: &str) {
    if consent_state() != 1 {
        return;
    }
    if !crate::server_offer::feature_on_global(crate::server_offer::ServerFeature::Reports) {
        eprintln!("[logship] this server takes no shared logs; the previous session's log stays here");
        return;
    }
    // Self-updater relaunch child: rotation was skipped (we inherit the parent's log
    // handle), so `.old.log` still holds the session the PARENT already shipped a
    // minute ago. Re-shipping it produced duplicate uploads on every update day.
    if std::env::var_os("TINHAT_RELAUNCH").is_some() {
        eprintln!("[logship] update-relaunch child — previous session already shipped by the parent; skipping");
        return;
    }
    if !prev_log_is(user, prev_owner()) {
        eprintln!("[logship] the previous session's log isn't this account's alone; it stays here");
        return;
    }
    let Some((base, token)) = destination() else {
        eprintln!("[logship] no server to send the previous session's log to; it stays here");
        return;
    };
    let old = old_log_path();
    let Ok(raw) = std::fs::read_to_string(&old) else { return };
    if raw.trim().is_empty() {
        return;
    }
    let body = scrub(&raw);
    // Filename: localpart only, filesystem-safe, plus epoch seconds for uniqueness.
    let who = who_of(user);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let url = log_upload_url(&base, &format!("{who}-{ts}.log"));
    let ver = env!("CARGO_PKG_VERSION");
    crate::bridge::runtime().spawn(async move {
        let client = crate::proxy::http(None);
        match client
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "text/plain; charset=utf-8")
            .header("X-TinHat-Version", ver)
            .body(body)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {
                eprintln!("[logship] previous session log shipped ({url})");
            }
            Ok(r) => eprintln!("[logship] upload rejected: HTTP {} (server side not ready?)", r.status()),
            Err(e) => eprintln!("[logship] upload failed (ok, non-fatal): {e}"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_previous_log_goes_only_to_the_account_it_belongs_to() {
        assert!(prev_log_is("@ana:a.example", "@ana:a.example"));
        assert!(!prev_log_is("@bo:b.example", "@ana:a.example"), "another account's log stays here");
        assert!(!prev_log_is("@ana:a.example", ""), "a mixed (or unknown) log stays here");
        assert!(!prev_log_is("", ""));
    }

    #[test]
    fn logs_and_reports_go_to_the_users_own_server() {
        assert_eq!(log_upload_url("https://maplehouse.tinhat.me/api", "ana-1790000000.log"), "https://maplehouse.tinhat.me/api/logs/upload?name=ana-1790000000.log");
        // the file name is kept plain for the URL
        assert_eq!(log_upload_url("https://x.test/api", "a b/c?.log"), "https://x.test/api/logs/upload?name=a_b_c_.log");
        assert_eq!(who_of("@ana:maplehouse.tinhat.me"), "ana");
        assert_eq!(who_of("@:x"), "unknown");
        assert_eq!(who_of("@a/b:x"), "ab");
    }
}
