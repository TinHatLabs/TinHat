//! login_errors.rs — the sign-in card's one-liner for a failure (D08.6 / D08.5, QoL sweep 2026-11-13; the web's
//! src/core/matrix/errors.ts friendlyLoginText, the same rules — testvectors/login-errors.json is byte-identical on
//! both sides). Every answer is one plain sentence a person can act on; the raw error chain goes to the log only.
//! Order matters: a certificate failure arrives inside a "client error (Connect)" chain, so it is named before the
//! connection words, and the server's own gateway errors ("504 Gateway Timeout") before the timeout words.

fn has_any(l: &str, words: &[&str]) -> bool {
    words.iter().any(|w| l.contains(w))
}

/// A status as its own number in the text ("[503 / M_UNKNOWN]", "… 500"), never part of a longer number, a port
/// (":429", ":4290") nor an address.
/// Addresses in the text ("https://host:503/…") are skipped first: their port is no status.
fn has_status(l: &str, codes: &[&str]) -> bool {
    l.split_whitespace().filter(|w| !w.contains("://")).any(|w| {
        // a number right after ':' is a port ("127.0.0.1:429"), never a status
        let mut prev = ' ';
        let mut run = String::new();
        let mut after_colon = false;
        for c in w.chars().chain(std::iter::once(' ')) {
            if c.is_ascii_digit() {
                if run.is_empty() {
                    after_colon = prev == ':';
                }
                run.push(c);
            } else {
                if !run.is_empty() && !after_colon && codes.contains(&run.as_str()) {
                    return true;
                }
                run.clear();
            }
            prev = c;
        }
        false
    })
}
fn has_server_status(l: &str) -> bool {
    has_status(l, &["500", "502", "503", "504"])
}

/// The sign-in card's sentence for the raw failure text (errcode + message + status, or the SDK's error chain).
pub fn friendly_login_text(raw: &str) -> String {
    let l = raw.to_lowercase();
    let s = if has_any(&l, &["m_forbidden", "invalid username", "invalid password"]) {
        "Wrong username or password."
    } else if has_any(&l, &["m_limit_exceeded", "too many"]) || has_status(&l, &["429"]) {
        "Too many attempts — wait a minute and try again."
    } else if l.contains("m_user_deactivated") {
        "That account has been deactivated."
    } else if l.contains("m_user_locked") {
        "That account is locked — ask whoever runs the server."
    } else if l.contains("m_user_suspended") {
        "That account is suspended — ask whoever runs the server."
    } else if has_any(&l, &["certificate", "unknownissuer", "notvalidyet"]) && !l.contains("certificaterequired") {
        // a certificate the checks refused (rustls: "invalid peer certificate: …"); a TLS negotiation failure
        // ("fatal alert: HandshakeFailure", or the server asking for a CLIENT certificate: "CertificateRequired") is not a
        // trust problem and reads as "Can't reach the server"
        if has_any(&l, &["expired", "notvalidyet", "not valid yet", "not yet valid"]) {
            "The server's certificate looks out of date — check that this device's date and time are right."
        } else {
            "The server's certificate isn't trusted on this machine."
        }
    } else if has_any(&l, &["dns", "resolve", "no such host", "name_not_resolved", "failed to lookup address"]) {
        "Can't find that server — check the address."
    } else if has_any(&l, &["bad gateway", "service unavailable", "gateway timeout", "internal server error"]) {
        "The server is having trouble right now — try again in a few minutes."
    } else if has_any(
        &l,
        &["failed to fetch", "networkerror", "load failed", "timed out", "timeout", "connection refused", "unreachable", "connect", "network"],
    ) {
        "Can't reach the server — check your connection and the address."
    } else if has_server_status(&l) {
        "The server is having trouble right now — try again in a few minutes."
    } else if has_any(&l, &["not a valid homeserver", "well-known", "unsupported", "m_unrecognized"]) {
        "That doesn't look like a Matrix server."
    } else if l.contains("forbidden") {
        // a bare "403 Forbidden" (a proxy's page, no Matrix errcode) is not a wrong password
        "The server refused to sign you in — check the address, or ask whoever runs it."
    } else {
        "Sign-in didn't work — try again in a moment."
    };
    s.to_string()
}

/// A14.6: the request never reached `server` (create account, invites): one sentence, no reqwest chain.
pub fn unreachable_text(server: &str) -> String {
    let who = if server.trim().is_empty() { "the server" } else { server.trim() };
    format!("Couldn't reach {who} — check your connection and the address.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_errors_shared_vectors() {
        let v: serde_json::Value = serde_json::from_str(include_str!("testvectors/login-errors.json")).unwrap();
        let cases = v["cases"].as_array().unwrap();
        assert!(cases.len() >= 20);
        for c in cases {
            assert_eq!(friendly_login_text(c["raw"].as_str().unwrap()), c["want"].as_str().unwrap(), "{}", c["name"]);
        }
        for c in v["unreachable"].as_array().unwrap() {
            assert_eq!(unreachable_text(c["server"].as_str().unwrap()), c["want"].as_str().unwrap());
        }
    }

    #[test]
    fn never_shows_the_raw_chain() {
        let raw = "error sending request for url (https://x.example/): client error (SendRequest): weird internal thing";
        assert!(!friendly_login_text(raw).contains("x.example"));
    }
}
