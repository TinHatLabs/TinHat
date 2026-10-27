//! start_over.rs — "Lost your Security Key? Start over" (2026-10-25, LOOP.md item I). For someone with no Security Key
//! and no other signed-in device: before this they could sign in but never unlock or verify anything again, and
//! nobody could help (keys never leave people's devices). Starting over gives the account a new identity, a new key
//! backup and a new Security Key; messages from before stay locked unless a device that still has them turns up, and
//! people who verified this account see that it changed.
//!
//! The order is what keeps it safe (Astra review 2026-10-25). The SDK's one-call reset (Recovery::reset_identity)
//! deletes the key backup and the secret storage BEFORE the new identity is accepted, so a wrong password, a network
//! error or a server that wants more than a password would leave the old backup gone and no new identity. Here:
//!   1. the password, checked with a request that changes nothing (deleting an empty list of devices asks for it and
//!      deletes nothing); a server that wants more than a password, or a wrong one, stops here
//!   2. the new identity, published with the password — refused = nothing on the server changed
//!   3. only then the old backup goes and a new backup + secret storage + Security Key are made; a failure here leaves
//!      the new identity in place and Start over can simply be run again
//! One at a time per app (START_OVER_BUSY). Same steps as Tin Hat Web's startOver (core/matrix/crypto.ts).
use matrix_sdk::encryption::CrossSigningResetAuthType;
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::ruma::api::client::uiaa;
use matrix_sdk::Client;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, PartialEq)]
pub enum StartOverError {
    /// The server said no to the password. Nothing changed.
    WrongPassword,
    /// The server doesn't confirm this with just a password (another sign-in method, or more steps). Nothing changed.
    NoPassword,
    /// A Start over is already running.
    Busy,
    /// Failed before anything changed.
    Before(String),
    /// The new identity is in place, but the new backup / Security Key failed: running Start over again finishes it.
    After(String),
}

impl StartOverError {
    /// The sentence the dialog shows (the web's startOver says the same).
    pub fn note(&self) -> String {
        match self {
            Self::WrongPassword => "That's not your account password. Nothing was changed.".to_string(),
            Self::NoPassword => "Your server asks for more than your password here, so Tin Hat can't start over. \
                                 Nothing was changed."
                .to_string(),
            Self::Busy => "Already starting over — one moment.".to_string(),
            Self::Before(e) => format!("Couldn't start over: {e}. Nothing was changed."),
            Self::After(e) => format!("Starting over didn't finish: {e}. Press Start over again to finish it."),
        }
    }
}

static START_OVER_BUSY: AtomicBool = AtomicBool::new(false);

/// The check's request went through without the server asking for the password (the web says the same).
pub const NOT_ASKED: &str = "your server didn't ask for your password, so Tin Hat can't confirm it's you";

/// The way into Start over (VerifyDeviceDialog.qml; the web's START_OVER.link, byte for byte).
pub const LINK: &str = "Lost your Security Key? Start over";
/// The second way out, the Security Key (the chooser's own words, repeated on the screens below; the web's USE_KEY).
pub const USE_KEY: &str = "No other device? Use your Security Key";

/// Which verification screens carry the way out (USE_KEY, then LINK), fix/recovery-escape-desktop 2026-10-26 — the
/// web's offersStartOver (startOver.ts), same table. A friend on a phone saw only "Use another device": Start over sat
/// inside the Security Key panel (behind a button that asks for the very key they didn't have), and once they had
/// tried another device the waiting and cancelled screens offered only Cancel / Try again. The desktop was the same.
/// - the first screen (state ""), waiting for my other device (I asked) and cancelled: while this device is not
///   verified (on the first screen right under the Security Key option, no other click needed). A verified device
///   keeps it where 12.0.1 put it, inside the Security Key panel: one click further from an accidental reset
/// - never while another device's request is being answered (someone is there) or emoji are on screen.
pub fn offers_start_over(sas_state: &str, sas_incoming: bool, this_device_verified: bool) -> bool {
    if this_device_verified {
        return false;
    }
    match sas_state {
        "" => true,
        "waiting" => !sas_incoming,
        s => s == "cancelled",
    }
}

/// Is a Start over running now? (The dialog ignores a second press instead of disturbing the first.)
pub fn is_running() -> bool {
    START_OVER_BUSY.load(Ordering::SeqCst)
}

/// Holds START_OVER_BUSY for one run (released on every return path).
struct Running;
impl Running {
    fn take() -> Option<Running> {
        START_OVER_BUSY.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).ok().map(|_| Running)
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        START_OVER_BUSY.store(false, Ordering::SeqCst);
    }
}

fn password_auth(client: &Client, password: &str, session: Option<String>) -> Option<uiaa::AuthData> {
    let user = client.user_id()?.to_owned();
    let mut pw = uiaa::Password::new(uiaa::UserIdentifier::from(user), password.to_owned());
    pw.session = session;
    Some(uiaa::AuthData::Password(pw))
}

/// A flow the password alone completes (not "password, then something else").
fn password_alone(info: &uiaa::UiaaInfo) -> bool {
    info.flows.iter().any(|f| f.stages.len() == 1 && f.stages[0] == uiaa::AuthType::Password)
}

/// The server turned the password down: a user-interactive auth answer whose error is M_FORBIDDEN. Anything else (a
/// network error, another error code, a second stage asked for) is not blamed on the password.
fn password_rejected(info: Option<&uiaa::UiaaInfo>) -> bool {
    info.and_then(|i| i.auth_error.as_ref()).is_some_and(|e| matches!(e.kind, ErrorKind::Forbidden))
}

/// Is `password` this account's password? Asked with a request that changes nothing: deleting an empty list of
/// devices needs the password (user-interactive auth) and deletes nothing. A server that lets it through without
/// asking (a "recently signed in" grace period) can't confirm the password, so that stops too (Astra review 2).
pub async fn check_password(client: &Client, password: &str) -> Result<(), StartOverError> {
    let first = match client.delete_devices(&[], None).await {
        Ok(_) => return Err(StartOverError::Before(NOT_ASKED.into())),
        Err(e) => e,
    };
    let Some(info) = first.as_uiaa_response() else {
        return Err(StartOverError::Before(first.to_string()));
    };
    if !password_alone(info) {
        return Err(StartOverError::NoPassword);
    }
    let auth = password_auth(client, password, info.session.clone()).ok_or(StartOverError::Before("not signed in".into()))?;
    match client.delete_devices(&[], Some(auth)).await {
        Ok(_) => Ok(()),
        Err(e) if password_rejected(e.as_uiaa_response()) => Err(StartOverError::WrongPassword),
        Err(e) if e.as_uiaa_response().is_some() => Err(StartOverError::NoPassword), // more steps asked for
        Err(e) => Err(StartOverError::Before(e.to_string())),
    }
}

/// Start over (see the module comment for the order). Returns the new Security Key — shown once, never stored.
pub async fn start_over(client: &Client, password: &str) -> Result<String, StartOverError> {
    let _running = Running::take().ok_or(StartOverError::Busy)?;
    if password.is_empty() {
        return Err(StartOverError::WrongPassword);
    }
    // 1. the password (nothing changes)
    check_password(client, password).await?;

    // 2. the new identity, published with the password. Only a password the server turned down means nothing changed:
    //    any other failure here may come after the keys went up (the SDK uploads signatures next), so it's "didn't
    //    finish" — Start over again completes it (Astra review 2).
    let enc = client.encryption();
    match enc.reset_cross_signing().await {
        Ok(None) => {} // the server didn't ask again (the password was checked above)
        Ok(Some(handle)) => {
            let auth = match handle.auth_type() {
                CrossSigningResetAuthType::Uiaa(info) if password_alone(info) => {
                    password_auth(client, password, info.session.clone())
                }
                _ => {
                    handle.cancel().await;
                    return Err(StartOverError::NoPassword);
                }
            };
            if let Err(e) = handle.auth(auth).await {
                handle.cancel().await;
                let rejected = match &e {
                    matrix_sdk::Error::Http(h) => password_rejected(h.as_uiaa_response()),
                    _ => false,
                };
                return Err(if rejected { StartOverError::WrongPassword } else { StartOverError::After(e.to_string()) });
            }
        }
        Err(e) => return Err(StartOverError::After(e.to_string())),
    }

    // 3. the new identity is in place: now the old backup goes, and a new backup + secret storage + Security Key
    enc.backups().disable_and_delete().await.map_err(|e| StartOverError::After(e.to_string()))?;
    enc.recovery().enable().await.map_err(|e| StartOverError::After(e.to_string()))
}

// ---- TINHAT_STARTOVER_TEST=1 TINHAT_HS=… TINHAT_USER=… TINHAT_PASS=… ./tinhat --------------------------------------
// A real server, three fresh sign-ins of ONE throwaway account (one at a time: one store per process):
//   A  sets the account up (identity + Security Key A)
//   B  has neither: a wrong password changes nothing; the right one starts over (Security Key B)
//   C  key A no longer unlocks anything; key B does, and C ends up verified

async fn master_key(client: &Client) -> Option<String> {
    let me = client.user_id()?.to_owned();
    let id = client.encryption().request_user_identity(&me).await.ok()??;
    id.master_key().get_first_key().map(|k| k.to_base64())
}

async fn backup_version(client: &Client) -> Option<String> {
    use matrix_sdk::ruma::api::client::backup::get_latest_backup_info;
    client.send(get_latest_backup_info::v3::Request::new()).await.ok().map(|r| r.version)
}

async fn fully_verified(client: &Client) -> bool {
    matches!(client.encryption().cross_signing_status().await, Some(s) if s.has_master && s.has_self_signing && s.has_user_signing)
}

/// A lab server only: this resets the account's encryption (a local address or a *.test name).
fn lab_server(url: &str) -> bool {
    let host = url::Url::parse(url).ok().and_then(|u| u.host_str().map(|h| h.trim_end_matches('.').to_ascii_lowercase()));
    matches!(host.as_deref(), Some(h) if h == "127.0.0.1" || h == "localhost" || h == "::1" || h == "[::1]" || h.ends_with(".test"))
}

pub async fn headless_start_over_test() -> bool {
    let server = match crate::bridge::env_homeserver() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("  FAIL {e}");
            return false;
        }
    };
    if !lab_server(&server) {
        eprintln!("  FAIL refusing {server}: this gate resets an account's encryption — a lab server only (local or *.test)");
        return false;
    }
    let user = std::env::var("TINHAT_USER").unwrap_or_default();
    let pass = std::env::var("TINHAT_PASS").unwrap_or_default();
    let mut failures = 0;
    let mut check = |label: &str, ok: bool| {
        eprintln!("  {} {label}", if ok { "ok  " } else { "FAIL" });
        if !ok {
            failures += 1;
        }
    };
    let login = |tag: &'static str| {
        let (server, user, pass) = (server.clone(), user.clone(), pass.clone());
        async move {
            let c = crate::bridge::headless_login(&server, &user, &pass).await.map_err(|e| format!("{tag}: login: {e}"))?;
            c.sync_once(crate::bridge::sync_settings()).await.map_err(|e| format!("{tag}: sync: {e}"))?;
            Ok::<Client, String>(c)
        }
    };

    // A: the account's first identity + Security Key A (the first-device path, no password: MSC3967)
    let a = match login("A").await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[startover] {e}");
            return false;
        }
    };
    if master_key(&a).await.is_none() {
        if let Err(e) = a.encryption().bootstrap_cross_signing(None).await {
            eprintln!("[startover] A: bootstrap: {e}");
            return false;
        }
    }
    let key_a = match a.encryption().recovery().enable().await {
        Ok(k) => k,
        Err(e) => {
            eprintln!("[startover] A: recovery enable: {e} (use a fresh account)");
            return false;
        }
    };
    let (master_a, backup_a) = (master_key(&a).await, backup_version(&a).await);
    check("A: identity + backup + Security Key", master_a.is_some() && backup_a.is_some() && !key_a.is_empty());
    drop(a);

    // B: no key, no other device
    let b = match login("B").await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[startover] {e}");
            return false;
        }
    };
    check("B: starts unverified", !fully_verified(&b).await);
    let wrong = start_over(&b, "not-the-password").await;
    check("B: a wrong password is refused", wrong == Err(StartOverError::WrongPassword));
    check("B: … and changes nothing (same identity, same backup)", master_key(&b).await == master_a && backup_version(&b).await == backup_a);
    let key_b = match start_over(&b, &pass).await {
        Ok(k) => k,
        Err(e) => {
            check(&format!("B: start over ({})", e.note()), false);
            return false;
        }
    };
    let (master_b, backup_b) = (master_key(&b).await, backup_version(&b).await);
    check("B: a new Security Key", !key_b.is_empty() && key_b != key_a);
    check("B: a new identity", master_b.is_some() && master_b != master_a);
    check("B: a new key backup", backup_b.is_some() && backup_b != backup_a);
    check("B: this device is verified", fully_verified(&b).await);
    drop(b);

    // C: the old key is worthless now, the new one works
    let c = match login("C").await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[startover] {e}");
            return false;
        }
    };
    check("C: Security Key A no longer unlocks", c.encryption().recovery().recover(&key_a).await.is_err());
    let with_b = c.encryption().recovery().recover(&key_b).await;
    check(&format!("C: Security Key B unlocks{}", with_b.as_ref().err().map(|e| format!(" ({e})")).unwrap_or_default()), with_b.is_ok());
    check("C: verified by it", fully_verified(&c).await);

    eprintln!("[startover] {}", if failures == 0 { "PASS" } else { "FAIL" });
    failures == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dialog's sentences say plainly whether anything changed (the web's startOver notes are the same).
    #[test]
    fn notes_say_whether_anything_changed() {
        assert!(StartOverError::WrongPassword.note().contains("Nothing was changed"));
        assert!(StartOverError::NoPassword.note().contains("Nothing was changed"));
        assert!(StartOverError::Before("x".into()).note().contains("Nothing was changed"));
        assert!(!StartOverError::After("x".into()).note().contains("Nothing was changed"));
        assert_eq!(StartOverError::After("x".into()).note(), "Starting over didn't finish: x. Press Start over again to finish it.");
        assert_eq!(StartOverError::WrongPassword.note(), "That's not your account password. Nothing was changed.");
    }

    /// Only a flow the password alone completes counts; "password, then something else" is not one (Astra 4).
    #[test]
    fn password_alone_flows() {
        let info = |flows: Vec<Vec<uiaa::AuthType>>| {
            let mut i = uiaa::UiaaInfo::new(flows.into_iter().map(uiaa::AuthFlow::new).collect());
            i.session = Some("S".into());
            i
        };
        assert!(password_alone(&info(vec![vec![uiaa::AuthType::Password]])));
        assert!(password_alone(&info(vec![vec![uiaa::AuthType::Sso], vec![uiaa::AuthType::Password]])));
        assert!(!password_alone(&info(vec![vec![uiaa::AuthType::Password, uiaa::AuthType::EmailIdentity]])));
        assert!(!password_alone(&info(vec![vec![uiaa::AuthType::Sso]])));
    }

    #[test]
    fn only_a_lab_server() {
        assert!(lab_server("http://127.0.0.1:8008"));
        assert!(lab_server("https://chat.crew.test"));
        assert!(!lab_server("https://chat.example.org"));
        assert!(!lab_server("https://name.tinhat.me"));
        assert!(!lab_server("https://test.example.com"));
    }

    /// The web's startOver.test.ts "the way out is on every screen a new device can end up on", same cases.
    #[test]
    fn the_way_out_is_on_every_screen() {
        let at = |s: &str| offers_start_over(s, false, false);
        // the first screen, no other click (a verified device: only inside the Security Key panel, as in 12.0.1)
        assert!(at(""));
        assert!(!offers_start_over("", false, true));
        // tried another device: waiting on it, or it was cancelled / timed out / nothing could vouch
        assert!(at("waiting"));
        assert!(at("cancelled"));
        assert!(offers_start_over("cancelled", true, false));
        // not while someone is there (another device asks), emoji are compared or keys are being made
        assert!(!offers_start_over("waiting", true, false));
        for s in ["emoji", "confirming", "done", "recovery-working", "recovery-done"] {
            assert!(!at(s), "{s}");
        }
        // a verified device has nothing to escape from
        assert!(!offers_start_over("waiting", false, true));
        assert!(!offers_start_over("cancelled", false, true));
    }

    /// The two ways out keep the web's words (startOver.ts USE_KEY / START_OVER.link), and the dialog says them.
    #[test]
    fn the_two_ways_out_keep_the_webs_words() {
        assert_eq!(USE_KEY, "No other device? Use your Security Key");
        assert_eq!(LINK, "Lost your Security Key? Start over");
        let qml = include_str!("../qml/VerifyDeviceDialog.qml");
        assert!(qml.contains(&format!("\"{USE_KEY}\"")));
        assert!(qml.contains(&format!("\"{LINK}\"")));
    }

    /// Two at once is refused; the guard is released when a run ends.
    #[test]
    fn one_at_a_time() {
        let first = Running::take();
        assert!(first.is_some());
        assert!(Running::take().is_none());
        drop(first);
        assert!(Running::take().is_some());
    }
}
