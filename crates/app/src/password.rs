//! password.rs — a forgotten password and a changed one (selfhost; 2026-10-22: "no email on self-hosted Tin Hat
//! servers, so a forgotten password must have a way back"). SHARED with Tin Hat Web (the words, the request bodies and
//! the error mapping, line for line; the web builds the same faces to the same contract). No Qt here: the faces live in
//! bridge.rs (the sign-in screen's "Forgot your password?") and settings.rs (Settings → Devices & security → Password).
//!
//! **Forgot your password?** A Tin Hat Server sends no email; its owner sets a new password from the server's dashboard
//! (People → Set a new password) and says so in its well-known: `dev.tinhat.server.account.password_reset = "owner"`
//! (server_offer.rs parses it with the rest of the offer). When the reset face opens, the app asks the typed server
//! (`https://<host>/.well-known/matrix/client`, 10 s, no auth) — "Checking <host>…" meanwhile. `"owner"` = no email field,
//! `owner_reset_text(host)` and only "Back to sign in"; anything else (no document, no key, another value, a network
//! error) = the email form, unchanged. `TINHAT_SERVER_OFFER` pins the answer for gates (the server is never asked).
//!
//! **Change password** (every server; standard Matrix user-interactive auth): `POST /_matrix/client/v3/account/password`
//! with `{"new_password", "logout_devices": false}` → 401 with `flows` + `session` → the same POST with
//! `auth = m.login.password` (identifier `m.id.user` = the full `@user:server`, the current password, the session) —
//! only when a flow is exactly `["m.login.password"]`; otherwise "This server wants another way to confirm it's you:
//! change your password where you signed up." and the password is never sent. 200 → changed (the other devices stay
//! signed in); the second request's 401 with errcode M_FORBIDDEN → not the current password; anything else on
//! either request → the server's `error`, or "The server didn't take the new password (HTTP <code>)." Passwords are
//! never logged and never kept after the request.
use std::time::Duration;

use serde_json::{json, Value};

/// What the well-known says about forgotten passwords (`dev.tinhat.server.account.password_reset`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PasswordReset {
    /// today's behaviour: the email form (no answer, no key, any other value)
    #[default]
    Email,
    /// `"owner"`: this server sends no email; its owner sets a new password from the dashboard (People)
    Owner,
}

impl PasswordReset {
    /// The value as announced (`"owner"`) / as the reset face's mode reads it.
    pub fn name(self) -> &'static str {
        match self {
            PasswordReset::Email => "email",
            PasswordReset::Owner => "owner",
        }
    }
}

/// The new password's minimum (the create-account face's rule).
pub const MIN_LEN: usize = 8;

/// How long the reset face waits for the typed server's well-known.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

// ---- the sign-in screen: "Forgot your password?" ----------------------------------------------------------------

/// The host of what the Server field says (the moved check's rule, bridge.rs check_moved / the web's hostOf): trimmed,
/// no scheme, no trailing slash, lowercase. None = nothing usable typed (empty, a path, a space).
pub fn server_host(typed: &str) -> Option<String> {
    let host = typed.trim().trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/').to_ascii_lowercase();
    if host.is_empty() || host.contains('/') || host.contains(' ') {
        None
    } else {
        Some(host)
    }
}

/// The status line while the face asks the server.
pub fn checking_text(host: &str) -> String {
    format!("Checking {host}…")
}

/// The reset face on a server whose owner sets new passwords (exact words, both apps).
pub fn owner_reset_text(host: &str) -> String {
    format!(
        "This server doesn't send email. Ask the person who runs {host} to set a new password for you: they do it on \
         the server's dashboard, under People. Sign in with the password they give you, then choose your own in \
         Settings → Devices & security → Change password."
    )
}

/// What the face shows for the well-known's answer: `Some((status, body))`, or None for a network error / timeout.
/// Only a 200 whose JSON carries `"owner"` (server_offer::parse_offer's rule) is the owner face.
pub fn reset_of_answer(answer: Option<(u16, &str)>) -> PasswordReset {
    match answer {
        Some((200, body)) => match serde_json::from_str::<Value>(body) {
            Ok(v) => crate::server_offer::parse_offer(&v).password_reset,
            Err(_) => PasswordReset::Email,
        },
        _ => PasswordReset::Email,
    }
}

/// Ask the typed server (`https://<host>/.well-known/matrix/client`, 10 s, no auth) how it resets passwords.
pub async fn read_reset(host: &str) -> PasswordReset {
    let url = format!("https://{host}/.well-known/matrix/client");
    let answer = match crate::proxy::http(Some(CHECK_TIMEOUT)).get(&url).send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Some((status, body))
        }
        Err(e) => {
            eprintln!("[password] couldn't read {url} ({e}) — the email form");
            None
        }
    };
    let r = reset_of_answer(answer.as_ref().map(|(s, b)| (*s, b.as_str())));
    eprintln!("[password] {host}: password reset by {}", r.name());
    r
}

// ---- Settings → Devices & security → Password: "Change password" -------------------------------------------------

/// The words of the change (the dialog's subtitle / the toast).
pub const CHANGED_TEXT: &str = "Password changed. Your other devices stay signed in.";
pub const WRONG_CURRENT_TEXT: &str = "That isn't your current password.";
/// A request that never got an answer (agreed for both apps 2026-10-22; the web uses the same words).
pub const UNREACHABLE_TEXT: &str = "Couldn't reach your server. Try again in a moment.";

/// "Change password" is enabled: a current password, a new one of at least 8 characters (UTF-16 units, as the web's
/// `length`), typed the same twice.
pub fn change_ready(current: &str, new_password: &str, again: &str) -> bool {
    !current.is_empty() && new_password.encode_utf16().count() >= MIN_LEN && new_password == again
}

/// This server's user-interactive auth for the change has no flow the password alone completes (SSO, several steps):
/// refused without sending the password.
pub const UNSUPPORTED_TEXT: &str = "This server wants another way to confirm it's you: change your password where you signed up.";

/// The request body: the first POST (`auth` None) and the second (`Some((user, current, session))`; a challenge
/// without a session id gets an `auth` without one).
pub fn change_body(new_password: &str, auth: Option<(&str, &str, Option<&str>)>) -> Value {
    let mut body = json!({ "new_password": new_password, "logout_devices": false });
    if let Some((user, current, session)) = auth {
        let mut a = json!({
            "type": "m.login.password",
            "identifier": { "type": "m.id.user", "user": user },
            "password": current,
        });
        if let Some(session) = session {
            a["session"] = Value::String(session.to_string());
        }
        body["auth"] = a;
    }
    body
}

/// How a change ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangeOutcome {
    Changed,
    WrongCurrent,
    /// the words to show
    Failed(String),
}

impl ChangeOutcome {
    /// What QML reads (`passwordOutcome`): "changed" | "wrong" | "failed".
    pub fn kind(&self) -> &'static str {
        match self {
            ChangeOutcome::Changed => "changed",
            ChangeOutcome::WrongCurrent => "wrong",
            ChangeOutcome::Failed(_) => "failed",
        }
    }
    /// The sentence for it.
    pub fn text(&self) -> String {
        match self {
            ChangeOutcome::Changed => CHANGED_TEXT.to_string(),
            ChangeOutcome::WrongCurrent => WRONG_CURRENT_TEXT.to_string(),
            ChangeOutcome::Failed(t) => t.clone(),
        }
    }
}

/// The server's `error` text, or "The server didn't take the new password (HTTP <code>)."
pub fn failure_text(status: u16, body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(Value::as_str).map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("The server didn't take the new password (HTTP {status})."))
}

/// What the first answer means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FirstStep {
    /// changed without asking for the current password
    Done,
    /// a user-interactive challenge the password alone completes: answer it (its session id, when it gave one)
    Password { session: Option<String> },
    Failed(String),
}

/// JS truthiness of a JSON value (the web's `!e.body.flows`).
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

/// A flow whose `stages` is exactly `["m.login.password"]` (the web: `stages.length === 1 && stages[0] === …`).
fn password_only(flows: &Value) -> bool {
    flows.as_array().is_some_and(|fs| {
        fs.iter().any(|f| {
            f.get("stages").and_then(Value::as_array).is_some_and(|st| st.len() == 1 && st[0].as_str() == Some("m.login.password"))
        })
    })
}

/// The first answer: 2xx = done. A 401 that is a user-interactive challenge (it has `flows`): the password step only
/// when a flow is exactly `["m.login.password"]`, else UNSUPPORTED_TEXT (the password is never sent). Anything else
/// (a 401 without `flows`, i.e. a dead token; another status) = the server's words.
pub fn first_answer(status: u16, body: &str) -> FirstStep {
    if (200..300).contains(&status) {
        return FirstStep::Done;
    }
    let v = serde_json::from_str::<Value>(body).ok();
    if status == 401 {
        if let Some(v) = v.as_ref().filter(|v| truthy(v.get("flows"))) {
            if !password_only(&v["flows"]) {
                return FirstStep::Failed(UNSUPPORTED_TEXT.to_string());
            }
            let session = v.get("session").and_then(Value::as_str).map(str::to_owned);
            return FirstStep::Password { session };
        }
    }
    FirstStep::Failed(failure_text(status, body))
}

/// The second answer: 2xx = changed; 401 with errcode M_FORBIDDEN = not the current password (Synapse's wrong
/// password: 401, the challenge again + M_FORBIDDEN "Invalid username or password"); anything else = the server's words
/// — a 403 too, even with M_FORBIDDEN (Synapse's "Password change disabled", handlers/set_password.py).
pub fn second_answer(status: u16, body: &str) -> ChangeOutcome {
    if (200..300).contains(&status) {
        return ChangeOutcome::Changed;
    }
    let errcode = serde_json::from_str::<Value>(body).ok().and_then(|v| v.get("errcode").and_then(Value::as_str).map(str::to_owned));
    if status == 401 && errcode.as_deref() == Some("M_FORBIDDEN") {
        return ChangeOutcome::WrongCurrent;
    }
    ChangeOutcome::Failed(failure_text(status, body))
}

/// The two steps over any transport (`post(body)` → Some((status, body)), None = no answer): change_password's HTTP,
/// the tests' recorder. The current password goes out only in the second body, and only for a password-only flow.
pub async fn change_with<F, Fut>(mut post: F, user: &str, current: &str, new_password: &str) -> ChangeOutcome
where
    F: FnMut(Value) -> Fut,
    Fut: std::future::Future<Output = Option<(u16, String)>>,
{
    let Some((status, text)) = post(change_body(new_password, None)).await else {
        return ChangeOutcome::Failed(UNREACHABLE_TEXT.to_string());
    };
    let session = match first_answer(status, &text) {
        FirstStep::Done => {
            eprintln!("[password] change: changed (no current password asked)");
            return ChangeOutcome::Changed;
        }
        FirstStep::Failed(t) => {
            eprintln!("[password] change: refused at the first step (HTTP {status})");
            return ChangeOutcome::Failed(t);
        }
        FirstStep::Password { session } => session,
    };
    let Some((status, text)) = post(change_body(new_password, Some((user, current, session.as_deref())))).await else {
        return ChangeOutcome::Failed(UNREACHABLE_TEXT.to_string());
    };
    let out = second_answer(status, &text);
    eprintln!("[password] change: {} (HTTP {status})", out.kind());
    out
}

/// The two-step change against the signed-in account's homeserver (`homeserver` = its base URL, `token` = the session's
/// access token, `user` = the full `@user:server`). Nothing of either password is logged.
pub async fn change_password(homeserver: &str, token: &str, user: &str, current: &str, new_password: &str) -> ChangeOutcome {
    let url = format!("{}/_matrix/client/v3/account/password", homeserver.trim_end_matches('/'));
    let http = crate::proxy::http(Some(Duration::from_secs(30)));
    let post = |body: Value| {
        let req = http.post(&url).bearer_auth(token).header("Content-Type", "application/json").body(body.to_string());
        async move {
            match req.send().await {
                Ok(r) => {
                    let status = r.status().as_u16();
                    Some((status, r.text().await.unwrap_or_default()))
                }
                Err(e) => {
                    eprintln!("[password] change: no answer ({})", e.without_url());
                    None
                }
            }
        }
    };
    change_with(post, user, current, new_password).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_typed_server_s_host() {
        assert_eq!(server_host("maplehouse.tinhat.me").as_deref(), Some("maplehouse.tinhat.me"));
        assert_eq!(server_host("  https://MapleHouse.tinhat.me/ ").as_deref(), Some("maplehouse.tinhat.me"));
        assert_eq!(server_host("http://localhost:8008").as_deref(), Some("localhost:8008"));
        for bad in ["", "   ", "https://", "x.test/path", "two words"] {
            assert_eq!(server_host(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_words_of_the_reset_face() {
        assert_eq!(checking_text("alpha.local"), "Checking alpha.local…");
        assert_eq!(
            owner_reset_text("maplehouse.tinhat.me"),
            "This server doesn't send email. Ask the person who runs maplehouse.tinhat.me to set a new password for you: \
             they do it on the server's dashboard, under People. Sign in with the password they give you, then choose your \
             own in Settings → Devices & security → Change password."
        );
    }

    #[test]
    fn only_an_owner_answer_is_the_owner_face() {
        let doc = |pr: Value| json!({ "dev.tinhat.server": { "server": "0.2.22", "account": { "password_reset": pr } } }).to_string();
        assert_eq!(reset_of_answer(Some((200, &doc(json!("owner"))))), PasswordReset::Owner);
        // anything else = the email form: another value, not a string, no key, no document, an error, a timeout
        assert_eq!(reset_of_answer(Some((200, &doc(json!("email"))))), PasswordReset::Email);
        assert_eq!(reset_of_answer(Some((200, &doc(json!("Owner"))))), PasswordReset::Email);
        assert_eq!(reset_of_answer(Some((200, &doc(json!(true))))), PasswordReset::Email);
        assert_eq!(reset_of_answer(Some((200, &doc(Value::Null)))), PasswordReset::Email);
        assert_eq!(reset_of_answer(Some((200, r#"{"dev.tinhat.server": {"server": "0.2.21"}}"#))), PasswordReset::Email);
        assert_eq!(reset_of_answer(Some((200, r#"{"m.homeserver": {"base_url": "https://x"}}"#))), PasswordReset::Email);
        assert_eq!(reset_of_answer(Some((200, "<html>"))), PasswordReset::Email);
        assert_eq!(reset_of_answer(Some((404, &doc(json!("owner"))))), PasswordReset::Email);
        assert_eq!(reset_of_answer(Some((500, ""))), PasswordReset::Email);
        assert_eq!(reset_of_answer(None), PasswordReset::Email);
    }

    #[test]
    fn change_password_is_enabled_when() {
        assert!(change_ready("old", "12345678", "12345678"));
        assert!(!change_ready("", "12345678", "12345678"), "no current password");
        assert!(!change_ready("old", "1234567", "1234567"), "7 characters");
        assert!(!change_ready("old", "12345678", "12345679"), "they differ");
        assert!(change_ready("old", "ééééćććć", "ééééćććć"), "characters, not bytes");
    }

    #[test]
    fn the_two_request_bodies() {
        assert_eq!(change_body("n3w-Pass", None), json!({ "new_password": "n3w-Pass", "logout_devices": false }));
        assert_eq!(
            change_body("n3w-Pass", Some(("@ana:maplehouse.tinhat.me", "old-Pass", Some("sEsSiOn")))),
            json!({
                "new_password": "n3w-Pass",
                "logout_devices": false,
                "auth": {
                    "type": "m.login.password",
                    "identifier": { "type": "m.id.user", "user": "@ana:maplehouse.tinhat.me" },
                    "password": "old-Pass",
                    "session": "sEsSiOn"
                }
            })
        );
        // a challenge without a session id: the auth without one (the web sends `session: undefined`)
        assert!(change_body("n", Some(("@a:b", "o", None)))["auth"].get("session").is_none());
    }

    // Synapse's answers, as it sends them (handlers/auth.py check_ui_auth: a failed step merges M_FORBIDDEN into the
    // challenge, 401)
    const CHALLENGE: &str = r#"{"session":"xyz","flows":[{"stages":["m.login.password"]}],"params":{}}"#;
    const WRONG: &str = r#"{"session":"xyz","flows":[{"stages":["m.login.password"]}],"params":{},"completed":[],"errcode":"M_FORBIDDEN","error":"Invalid username or password"}"#;

    #[test]
    fn the_first_answer() {
        assert_eq!(first_answer(401, CHALLENGE), FirstStep::Password { session: Some("xyz".into()) });
        assert_eq!(first_answer(200, "{}"), FirstStep::Done);
        // a password-only flow among others is enough; one without a session id still answers
        assert_eq!(
            first_answer(401, r#"{"flows":[{"stages":["m.login.sso"]},{"stages":["m.login.password"]}]}"#),
            FirstStep::Password { session: None }
        );
        // no flow the password alone completes: refused (SSO only, password + another step, empty, not an array)
        for flows in [
            r#"[{"stages":["m.login.sso"]}]"#,
            r#"[{"stages":["m.login.password","m.login.recaptcha"]}]"#,
            r#"[{"stages":[]}]"#,
            r#"[]"#,
            r#"{"stages":["m.login.password"]}"#,
            r#"[{"stages":"m.login.password"}]"#,
        ] {
            let body = format!(r#"{{"session":"xyz","flows":{flows}}}"#);
            assert_eq!(first_answer(401, &body), FirstStep::Failed(UNSUPPORTED_TEXT.into()), "{flows}");
        }
        assert_eq!(UNSUPPORTED_TEXT, "This server wants another way to confirm it's you: change your password where you signed up.");
        // a 401 that isn't a challenge (a dead token), another status: the server's words
        assert_eq!(
            first_answer(401, r#"{"errcode":"M_UNKNOWN_TOKEN","error":"Invalid access token passed."}"#),
            FirstStep::Failed("Invalid access token passed.".into())
        );
        assert_eq!(first_answer(401, r#"{"session":"xyz","flows":null}"#), FirstStep::Failed("The server didn't take the new password (HTTP 401).".into()));
        assert_eq!(
            first_answer(400, r#"{"errcode":"M_PASSWORD_TOO_SHORT","error":"Password too short"}"#),
            FirstStep::Failed("Password too short".into())
        );
        assert_eq!(first_answer(502, "<html>bad gateway</html>"), FirstStep::Failed("The server didn't take the new password (HTTP 502).".into()));
    }

    #[test]
    fn the_second_answer_and_its_words() {
        assert_eq!(second_answer(200, "{}"), ChangeOutcome::Changed);
        // wrong = 401 with M_FORBIDDEN only
        assert_eq!(second_answer(401, WRONG), ChangeOutcome::WrongCurrent);
        assert_eq!(second_answer(401, r#"{"errcode":"M_FORBIDDEN","error":"Invalid password"}"#), ChangeOutcome::WrongCurrent);
        // a 403, even M_FORBIDDEN, is the server's own words (Synapse: password changes switched off, after the UIA passed)
        assert_eq!(
            second_answer(403, r#"{"errcode":"M_FORBIDDEN","error":"Password change disabled"}"#),
            ChangeOutcome::Failed("Password change disabled".into())
        );
        assert_eq!(second_answer(403, r#"{"errcode":"M_FORBIDDEN"}"#), ChangeOutcome::Failed("The server didn't take the new password (HTTP 403).".into()));
        assert_eq!(
            second_answer(401, r#"{"errcode":"M_UNKNOWN_TOKEN","error":"Invalid access token passed."}"#),
            ChangeOutcome::Failed("Invalid access token passed.".into())
        );
        assert_eq!(second_answer(403, r#"{"error":"Password change disabled"}"#), ChangeOutcome::Failed("Password change disabled".into()));
        assert_eq!(second_answer(401, r#"{"session":"xyz","flows":[]}"#), ChangeOutcome::Failed("The server didn't take the new password (HTTP 401).".into()));
        assert_eq!(
            second_answer(400, r#"{"errcode":"M_WEAK_PASSWORD","error":"Password does not contain any digits"}"#),
            ChangeOutcome::Failed("Password does not contain any digits".into())
        );
        assert_eq!(second_answer(500, ""), ChangeOutcome::Failed("The server didn't take the new password (HTTP 500).".into()));
        assert_eq!(second_answer(429, r#"{"error":"   "}"#), ChangeOutcome::Failed("The server didn't take the new password (HTTP 429).".into()));
        assert_eq!(ChangeOutcome::Changed.text(), "Password changed. Your other devices stay signed in.");
        assert_eq!(ChangeOutcome::WrongCurrent.text(), "That isn't your current password.");
        assert_eq!(ChangeOutcome::Changed.kind(), "changed");
        assert_eq!(ChangeOutcome::WrongCurrent.kind(), "wrong");
        assert_eq!(ChangeOutcome::Failed("x".into()).kind(), "failed");
    }

    /// The whole change over a recording transport: what goes out, in which order, and what comes of each answer.
    fn run(answers: &[Option<(u16, &str)>]) -> (ChangeOutcome, Vec<Value>) {
        use std::cell::RefCell;
        use std::future::Future;
        use std::task::{Context, Poll, Waker};
        let sent = RefCell::new(Vec::new());
        let mut next = answers.iter().cloned();
        let post = |body: Value| {
            sent.borrow_mut().push(body);
            std::future::ready(next.next().flatten().map(|(s, b)| (s, b.to_string())))
        };
        let out = {
            let mut fut = std::pin::pin!(change_with(post, "@ana:maplehouse.tinhat.me", "old-Secret-1", "new-Secret-2"));
            match fut.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
                Poll::Ready(o) => o,
                Poll::Pending => panic!("the recording transport answers at once"),
            }
        };
        (out, sent.into_inner())
    }
    fn carries_current(bodies: &[Value]) -> bool {
        bodies.iter().any(|b| b.to_string().contains("old-Secret-1"))
    }

    #[test]
    fn the_whole_change_over_a_recording_transport() {
        // the usual: challenge → the password step → 200
        let (out, sent) = run(&[Some((401, CHALLENGE)), Some((200, "{}"))]);
        assert_eq!(out, ChangeOutcome::Changed);
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0], change_body("new-Secret-2", None));
        assert!(!carries_current(&sent[..1]), "the first request never carries the current password");
        assert_eq!(sent[1], change_body("new-Secret-2", Some(("@ana:maplehouse.tinhat.me", "old-Secret-1", Some("xyz")))));
        // no password-only flow: refused WITHOUT sending the password (one request, no auth)
        let (out, sent) = run(&[Some((401, r#"{"session":"xyz","flows":[{"stages":["m.login.sso"]}]}"#)), Some((200, "{}"))]);
        assert_eq!(out, ChangeOutcome::Failed(UNSUPPORTED_TEXT.into()));
        assert_eq!(sent.len(), 1);
        assert!(!carries_current(&sent), "the password is never sent");
        assert!(sent[0].get("auth").is_none());
        // the second request: 401 M_FORBIDDEN = wrong current password; 401 M_UNKNOWN_TOKEN and 403 M_FORBIDDEN = the
        // server's words
        assert_eq!(
            run(&[Some((401, CHALLENGE)), Some((403, r#"{"errcode":"M_FORBIDDEN","error":"Password change disabled"}"#))]).0,
            ChangeOutcome::Failed("Password change disabled".into())
        );
        assert_eq!(run(&[Some((401, CHALLENGE)), Some((401, WRONG))]).0, ChangeOutcome::WrongCurrent);
        assert_eq!(
            run(&[Some((401, CHALLENGE)), Some((401, r#"{"errcode":"M_UNKNOWN_TOKEN","error":"Invalid access token passed."}"#))]).0,
            ChangeOutcome::Failed("Invalid access token passed.".into())
        );
        // a dead token at the first request: the server's words, the password never sent
        let (out, sent) = run(&[Some((401, r#"{"errcode":"M_UNKNOWN_TOKEN","error":"Invalid access token passed."}"#))]);
        assert_eq!(out, ChangeOutcome::Failed("Invalid access token passed.".into()));
        assert!(!carries_current(&sent));
        // no answer at either step
        assert_eq!(run(&[None]).0, ChangeOutcome::Failed(UNREACHABLE_TEXT.into()));
        assert_eq!(run(&[Some((401, CHALLENGE)), None]).0, ChangeOutcome::Failed(UNREACHABLE_TEXT.into()));
        // a server that changes it outright
        let (out, sent) = run(&[Some((200, "{}"))]);
        assert_eq!(out, ChangeOutcome::Changed);
        assert_eq!(sent.len(), 1);
    }
}
