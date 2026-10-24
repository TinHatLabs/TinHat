//! InviteController — registration invite codes for the server's operators (10.4).
//!
//! Why (2026-09-19): an admin had nowhere to create an invite token that lets a new person create
//! their first account on the service. The homeserver
//! runs `registration_requires_token`, so a new account needs a Synapse registration token; this
//! controller talks to the Tin Hat server's `/api/admin/invites`, which
//! mints / lists / revokes those tokens with ITS admin credentials after checking that the caller
//! is an operator (a Synapse server admin, or listed in `invite-operators.json`). The client never
//! sees an admin token: it sends its own Matrix access token, like the moderation endpoint.
//!
//! `probe()` runs once after sign-in: 200 = operator (Settings grows an Invites pane), 403 = not.
//! The sign-in screen's "Create an account" redeems a code (bridge.rs `create_account`).

use core::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;
use serde_json::{json, Value};

/// Every round trip is bounded — one click, one answer.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);

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
        #[qproperty(bool, is_operator)] //     QML: isOperator — the signed-in user may manage invites
        #[qproperty(bool, probed)] //          QML: probed — the operator check has an answer
        #[qproperty(bool, busy)] //            QML: busy — a request is in flight
        #[qproperty(i32, rev)] //              QML: rev — bumps whenever invitesJson / isOperator change
        #[qproperty(QString, invites_json)] // QML: invitesJson — [{code,usesAllowed,pending,completed,expiresAt,active}]
        #[qproperty(QString, server_name)] //  QML: serverName — the server the codes create accounts on
        #[qproperty(QString, status)] //       QML: status — last success text
        #[qproperty(i32, status_rev)] //       QML: statusRev — bumps on every status set
        #[qproperty(QString, last_error)] //   QML: lastError — last failure text (user-facing)
        #[qproperty(i32, error_rev)] //        QML: errorRev — bumps on every lastError set
        type InviteController = super::InviteControllerRust;

        /// QML: probe() — am I an operator? Loads the list when yes. Quiet on network trouble.
        #[qinvokable]
        fn probe(self: Pin<&mut InviteController>);
        /// QML: refresh() — reload the list (errors toast).
        #[qinvokable]
        fn refresh(self: Pin<&mut InviteController>);
        /// QML: create(uses, days) — mint a code: uses 0 = unlimited, days 0 = never expires.
        #[qinvokable]
        fn create(self: Pin<&mut InviteController>, uses: i32, days: i32);
        /// QML: revoke(code) — delete a code; anyone holding it can no longer register.
        #[qinvokable]
        fn revoke(self: Pin<&mut InviteController>, code: &QString);
        /// QML: reset() — forget everything (sign-out).
        #[qinvokable]
        fn reset(self: Pin<&mut InviteController>);
    }

    impl cxx_qt::Threading for InviteController {}
}

#[derive(Default)]
pub struct InviteControllerRust {
    is_operator: bool,
    probed: bool,
    busy: bool,
    rev: i32,
    invites_json: QString,
    server_name: QString,
    status: QString,
    status_rev: i32,
    last_error: QString,
    error_rev: i32,
}

/// What a request came back with, for the Qt thread to apply.
enum Reply {
    /// 200 with the list: operator, invites, server.
    List { invites: Value, server: String, note: String },
    /// 401 / 403: the account is not an operator (probe answers "no", actions say so).
    Forbidden,
    /// Anything else, with a sentence for the user.
    Failed(String),
}

fn bearer() -> Option<String> {
    crate::session::client().and_then(|c| c.access_token())
}

/// 10.5.2: the API's client carries `TinHat/<version> (<os>)` ([`crate::proxy::http`]).
fn http() -> matrix_sdk::reqwest::Client {
    crate::proxy::http(Some(TIMEOUT))
}

/// One request to `{proxy::base()}/admin/invites…`: (status, body).
async fn call(method: &str, path: &str, body: Option<Value>) -> Result<(u16, Value), String> {
    let Some(token) = bearer() else {
        return Err("Not signed in".to_string());
    };
    let url = format!("{}{}", crate::proxy::base(), path);
    let c = http();
    let mut req = match method {
        "POST" => c.post(&url),
        "DELETE" => c.delete(&url),
        _ => c.get(&url),
    };
    req = req.header("Authorization", format!("Bearer {token}"));
    if let Some(b) = body {
        req = req.header("Content-Type", "application/json").body(b.to_string());
    }
    let resp = req.send().await.map_err(|e| {
        if e.is_timeout() {
            "The server didn't answer in time".to_string()
        } else {
            format!("Couldn't reach the server: {e}")
        }
    })?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    Ok((status, serde_json::from_str::<Value>(&text).unwrap_or(Value::Null)))
}

fn server_error(status: u16, body: &Value) -> String {
    match body.get("error").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        Some(s) => s.to_string(),
        None => format!("The server said {status}"),
    }
}

/// GET the list; `note` is the success line to show first (after a create / revoke).
async fn fetch_list(note: String) -> Reply {
    match call("GET", "/admin/invites", None).await {
        Ok((200, v)) => Reply::List {
            invites: v.get("invites").cloned().unwrap_or_else(|| json!([])),
            server: v.get("server").and_then(Value::as_str).unwrap_or("").to_string(),
            note,
        },
        Ok((401 | 403, _)) => Reply::Forbidden,
        Ok((s, v)) => Reply::Failed(server_error(s, &v)),
        Err(e) => Reply::Failed(e),
    }
}

impl qobject::InviteController {
    fn bump(mut self: Pin<&mut Self>) {
        let r = *self.rev() + 1;
        self.as_mut().set_rev(r);
    }

    fn set_error_text(mut self: Pin<&mut Self>, e: &str) {
        self.as_mut().set_last_error(QString::from(e));
        let r = *self.error_rev() + 1;
        self.as_mut().set_error_rev(r);
    }

    fn set_status_text(mut self: Pin<&mut Self>, s: &str) {
        self.as_mut().set_status(QString::from(s));
        let r = *self.status_rev() + 1;
        self.as_mut().set_status_rev(r);
    }

    /// Apply a reply on the Qt thread. `quiet` = a failed probe stays silent (it runs for
    /// everyone at sign-in; a network hiccup is not the user's problem).
    fn apply(mut self: Pin<&mut Self>, reply: Reply, quiet: bool) {
        self.as_mut().set_busy(false);
        match reply {
            Reply::List { invites, server, note } => {
                self.as_mut().set_is_operator(true);
                self.as_mut().set_probed(true);
                self.as_mut().set_invites_json(QString::from(&invites.to_string()));
                self.as_mut().set_server_name(QString::from(&server));
                self.as_mut().bump();
                if !note.is_empty() {
                    self.as_mut().set_status_text(&note);
                }
            }
            Reply::Forbidden => {
                self.as_mut().set_is_operator(false);
                self.as_mut().set_probed(true);
                self.as_mut().set_invites_json(QString::default());
                self.as_mut().bump();
                if !quiet {
                    self.as_mut().set_error_text("Only a server admin can manage invite codes");
                }
            }
            Reply::Failed(e) => {
                if quiet {
                    eprintln!("[invites] probe: {e}");
                } else {
                    self.as_mut().set_error_text(&e);
                }
            }
        }
    }

    fn run(
        mut self: Pin<&mut Self>,
        quiet: bool,
        fut: impl std::future::Future<Output = Reply> + Send + 'static,
    ) {
        if *self.busy() {
            return;
        }
        self.as_mut().set_busy(true);
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let reply = fut.await;
            let _ = qt.queue(move |o| o.apply(reply, quiet));
        });
    }

    pub fn probe(self: Pin<&mut Self>) {
        self.run(true, fetch_list(String::new()));
    }

    pub fn refresh(self: Pin<&mut Self>) {
        self.run(false, fetch_list(String::new()));
    }

    pub fn create(self: Pin<&mut Self>, uses: i32, days: i32) {
        let uses = uses.clamp(0, 1000);
        let days = days.clamp(0, 3650);
        self.run(false, async move {
            match call("POST", "/admin/invites", Some(json!({ "uses": uses, "days": days }))).await {
                Ok((200, v)) => {
                    let code = v
                        .get("invite")
                        .and_then(|i| i.get("code"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    eprintln!("[invites] minted {code}");
                    fetch_list(format!("Invite {code} created")).await
                }
                Ok((401 | 403, _)) => Reply::Forbidden,
                Ok((s, v)) => Reply::Failed(server_error(s, &v)),
                Err(e) => Reply::Failed(e),
            }
        });
    }

    pub fn revoke(self: Pin<&mut Self>, code: &QString) {
        let code = code.to_string().trim().to_string();
        if code.is_empty() || !code.chars().all(|c| c.is_ascii_alphanumeric() || "._~-".contains(c)) {
            return;
        }
        self.run(false, async move {
            match call("DELETE", &format!("/admin/invites/{code}"), None).await {
                Ok((200, _)) | Ok((404, _)) => {
                    eprintln!("[invites] revoked {code}");
                    fetch_list(format!("Revoked {code}")).await
                }
                Ok((401 | 403, _)) => Reply::Forbidden,
                Ok((s, v)) => Reply::Failed(server_error(s, &v)),
                Err(e) => Reply::Failed(e),
            }
        });
    }

    pub fn reset(mut self: Pin<&mut Self>) {
        self.as_mut().set_is_operator(false);
        self.as_mut().set_probed(false);
        self.as_mut().set_busy(false);
        self.as_mut().set_invites_json(QString::default());
        self.as_mut().bump();
    }
}
