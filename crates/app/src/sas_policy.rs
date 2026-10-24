//! sas_policy.rs — the pure, Qt-free rules of interactive (emoji/SAS) device verification
//! that Tin Hat desktop and Tin Hat Web must apply IDENTICALLY (Critical Rule: one product).
//!
//! Mirror: tinhat-web `src/core/matrix/sasPolicy.ts` (+ its unit test). The test tables at
//! the bottom of this file are the shared vectors — change them on both sides or neither.
//!
//! S1 — when may the REQUESTER start the emoji comparison on its own?
//!   Element Classic (iOS) and Element Web advertise QR methods and let their user pick the
//!   method. Classic's picker screen only starts listening after it is pushed and never looks
//!   for a SAS that already exists, so a requester that auto-starts the moment Ready arrives
//!   leaves Classic on a dark "Scan QR code" screen; when its user then taps "Can't scan?",
//!   Classic starts its own SAS over ours and the Rust SDK cancels both (field case
//!   2026-09-25: "the screen just goes black and I cannot verify anything"). So:
//!     no m.sas.v1                                    → NoSas (nothing we can do)
//!     m.sas.v1 + m.qr_code.show.v1 or m.qr_code.scan.v1 → Wait (their user picks; we offer a button)
//!     otherwise                                      → Start (Tin Hat, Element X: SAS-only peers)
//!   m.reciprocate.v1 alone is NOT a QR offer: Tin Hat desktop <= 10.6.0 sends it by SDK default.
//!
//! S6 — what a cancelled flow says, by the wire cancel code (never the raw code as the lead).

use matrix_sdk::ruma::events::key::verification::VerificationMethod;

/// The requester's start decision for a peer's advertised methods (S1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SasStart {
    /// SAS-only peer: start the comparison ourselves, now (through the start guard).
    Start,
    /// The peer offers QR as well, so its user chooses: wait for their start, offer a button.
    Wait,
    /// The peer did not offer emoji verification at all.
    NoSas,
}

impl SasStart {
    /// The same spelling the web's `sasStartPolicy` returns ("start" | "wait" | "no-sas").
    pub fn as_str(self) -> &'static str {
        match self {
            SasStart::Start => "start",
            SasStart::Wait => "wait",
            SasStart::NoSas => "no-sas",
        }
    }
}

/// S1 over the wire method names (the shared, testable core).
pub fn sas_start_policy_names<S: AsRef<str>>(their_methods: &[S]) -> SasStart {
    let has = |m: &str| their_methods.iter().any(|x| x.as_ref() == m);
    if !has("m.sas.v1") {
        SasStart::NoSas
    } else if has("m.qr_code.show.v1") || has("m.qr_code.scan.v1") {
        SasStart::Wait
    } else {
        SasStart::Start
    }
}

/// S1 over the SDK's method list (VerificationRequestState::Ready's `their_methods`).
pub fn sas_start_policy(their_methods: &[VerificationMethod]) -> SasStart {
    let names: Vec<&str> = their_methods.iter().map(|m| m.as_str()).collect();
    sas_start_policy_names(&names)
}

/// S3 — the AUTOMATIC start (policy Start) waits this long for the rest of the sync batch before its
/// guard re-check (a peer's ready + its own start can land in one batch). The manual start does not
/// wait. Tin Hat Web: `SAS_AUTO_START_SETTLE_MS` (same value, same test).
pub const AUTO_START_SETTLE_MS: u64 = 300;

/// Method names only, for the `[sas]` log line (never Debug dumps of SDK state).
pub fn method_names(methods: &[VerificationMethod]) -> String {
    methods.iter().map(|m| m.as_str()).collect::<Vec<_>>().join(",")
}

/// What we advertise in BOTH the request and the ready (S4): exactly `[m.sas.v1]`, byte-identical
/// with Tin Hat Web (client.ts verificationMethods) and Element X. No Tin Hat build shows or scans
/// QR, and the SDK default added m.reciprocate.v1 (qrcode feature off), which said nothing true.
pub fn our_methods() -> Vec<VerificationMethod> {
    vec![VerificationMethod::SasV1]
}

// ---- Dialog strings the two apps share word for word ----------------------------------------

/// Incoming request just arrived (before the driver's first state).
pub const NOTE_INCOMING_ARRIVED: &str = "Another of your devices wants to verify. Accept to compare emoji.";
/// Incoming request, driver sees Requested.
pub const NOTE_INCOMING_REQUESTED: &str = "Another device is asking to verify. Accept to compare emoji.";
/// KeysExchanged.
pub const NOTE_EMOJI: &str = "Do these seven emoji match, in this order, on your other device?";
/// Requester: the SAS finished.
pub const NOTE_DONE_REQUESTER: &str = "This device is verified — decrypting your messages…";
/// Requester: the REQUEST finished before the SAS watcher reported (fallback).
pub const NOTE_DONE_REQUESTER_REQUEST: &str = "This device is verified.";
/// S7 — verifier (sasIncoming): it vouched for the OTHER device.
pub const NOTE_DONE_VERIFIER: &str = "Verified. Your other device can read your messages now.";
/// S1 NoSas.
pub const NOTE_NO_SAS: &str = "The other device did not offer emoji verification.";

/// S2 — the requester is waiting because the peer offers QR too (its user picks the method).
pub fn wait_note(peer: &str) -> String {
    format!(
        "{peer} is ready. On it, choose \"Verify by emoji\" — in the old Element app, tap \"Can't scan?\". Or start from here."
    )
}

/// S6 — the sentence a cancelled flow shows, by wire cancel code. `None` (or an empty code) =
/// no code known. Local failures that are not wire cancels ("Could not start: …") keep their
/// own text and never come through here.
pub fn cancel_note(code: Option<&str>) -> String {
    let code = code.map(str::trim).filter(|c| !c.is_empty());
    match code {
        Some("m.user") => "The verification was cancelled.".to_string(),
        Some("m.timeout") => "It took too long, so the check timed out. Try again.".to_string(),
        Some("m.unexpected_message") | Some("m.unknown_transaction") => {
            "The other device started its own comparison over this one, so both stopped. Try again, and on the other device wait for the emoji instead of starting."
                .to_string()
        }
        Some("m.key_mismatch") | Some("m.mismatched_sas") | Some("m.mismatched_commitment") => {
            "The emoji didn't match, so nothing was trusted.".to_string()
        }
        Some("m.accepted") => "Another of your devices answered first.".to_string(),
        Some("m.user_mismatch") | Some("m.invalid_message") | Some("m.unknown_method") => {
            "The other device sent something this app can't use. Try again, or use your Security Key.".to_string()
        }
        Some(other) => format!("The verification was cancelled ({other})."),
        None => "The verification was cancelled.".to_string(),
    }
}

// ---- F1 — the first device of a brand-new account (docs/VERIFY.md "First device", 2026-09-29) ----
// Web mirror: sasPolicy.ts firstDeviceSetup / FIRST_DEVICE_*.

/// F1 — does this sign-in set up the account's encryption itself? Only an account that has never had
/// any: no cross-signing identity on the server AND no key backup there. Never a guest. Every other
/// device is verified against one that already holds the keys (emoji), or with the Security Key.
pub fn first_device_setup(guest: bool, has_cross_signing: bool, backup_on_server: bool) -> bool {
    !guest && !has_cross_signing && !backup_on_server
}
/// F1 — how long the verify banner / sign-in prompt wait for the first-device check before showing anyway.
pub const FIRST_DEVICE_MAX_WAIT_MS: u64 = 30000;
/// F2 — the dialog title while the new Security Key is on screen.
pub const FIRST_DEVICE_TITLE: &str = "Your Security Key";
/// F2 — the note above the new Security Key.
pub const FIRST_DEVICE_NOTE: &str = "Encryption is set up and this device is verified — your other devices verify against it. Save this Security Key somewhere safe: it's shown once, and it's the only way back into your encrypted messages if you ever lose every signed-in device.";

#[cfg(test)]
mod tests {
    use super::*;

    /// SHARED VECTORS (F1) — identical in tinhat-web src/core/matrix/sasPolicy.test.ts.
    const FIRST: &[(&str, bool, bool, bool, bool)] = &[
        // (name, guest, has_cross_signing, backup_on_server, want)
        ("brand-new account", false, false, false, true),
        ("account has cross-signing", false, true, false, false),
        ("backup but no cross-signing", false, false, true, false),
        ("fully set up account", false, true, true, false),
        ("guest", true, false, false, false),
    ];

    #[test]
    fn first_device_setup_shared_vectors() {
        for (name, guest, xs, backup, want) in FIRST {
            assert_eq!(first_device_setup(*guest, *xs, *backup), *want, "{name}");
        }
    }

    #[test]
    fn first_device_strings_exact() {
        assert_eq!(FIRST_DEVICE_TITLE, "Your Security Key");
        assert_eq!(
            FIRST_DEVICE_NOTE,
            "Encryption is set up and this device is verified — your other devices verify against it. Save this Security Key somewhere safe: it's shown once, and it's the only way back into your encrypted messages if you ever lose every signed-in device."
        );
        assert_eq!(FIRST_DEVICE_MAX_WAIT_MS, 30000);
    }

    /// SHARED VECTORS (S1) — identical in tinhat-web src/core/matrix/sasPolicy.test.ts.
    const POLICY: &[(&[&str], &str)] = &[
        (&["m.sas.v1"], "start"),                      // Tin Hat Web, Element X
        (&["m.sas.v1", "m.reciprocate.v1"], "start"), // Tin Hat desktop <= 10.6.0
        (&["m.sas.v1", "m.qr_code.show.v1", "m.reciprocate.v1"], "wait"), // Element Web
        (&["m.sas.v1", "m.qr_code.show.v1", "m.qr_code.scan.v1", "m.reciprocate.v1"], "wait"), // Element Classic with a camera
        (&["m.sas.v1", "m.qr_code.scan.v1"], "wait"),
        (&["m.reciprocate.v1", "m.sas.v1"], "start"), // order-independent
        (&["m.qr_code.show.v1", "m.reciprocate.v1"], "no-sas"),
        (&[], "no-sas"),
        (&["m.sas.v1", "org.example.x"], "start"),    // unknown extra methods ignored
    ];

    #[test]
    fn start_policy_shared_vectors_by_name() {
        for (methods, want) in POLICY {
            assert_eq!(sas_start_policy_names(methods).as_str(), *want, "methods {methods:?}");
        }
    }

    #[test]
    fn start_policy_shared_vectors_through_the_sdk_enum() {
        // The same vectors through ruma's VerificationMethod (what Ready's their_methods carries),
        // so a spelling drift between the enum and the wire names can't hide.
        for (methods, want) in POLICY {
            let ms: Vec<VerificationMethod> = methods.iter().map(|m| VerificationMethod::from(*m)).collect();
            assert_eq!(sas_start_policy(&ms).as_str(), *want, "methods {methods:?}");
        }
    }

    #[test]
    fn we_advertise_exactly_sas_v1() {
        let ours = our_methods();
        let names: Vec<&str> = ours.iter().map(|m| m.as_str()).collect();
        assert_eq!(names, vec!["m.sas.v1"]);
        assert_eq!(method_names(&our_methods()), "m.sas.v1");
    }

    /// SHARED VECTORS (S6) — identical in tinhat-web src/core/matrix/sasPolicy.test.ts.
    const CANCEL: &[(Option<&str>, &str)] = &[
        (Some("m.user"), "The verification was cancelled."),
        (Some("m.timeout"), "It took too long, so the check timed out. Try again."),
        (Some("m.unexpected_message"), "The other device started its own comparison over this one, so both stopped. Try again, and on the other device wait for the emoji instead of starting."),
        (Some("m.unknown_transaction"), "The other device started its own comparison over this one, so both stopped. Try again, and on the other device wait for the emoji instead of starting."),
        (Some("m.key_mismatch"), "The emoji didn't match, so nothing was trusted."),
        (Some("m.mismatched_sas"), "The emoji didn't match, so nothing was trusted."),
        (Some("m.mismatched_commitment"), "The emoji didn't match, so nothing was trusted."),
        (Some("m.accepted"), "Another of your devices answered first."),
        (Some("m.user_mismatch"), "The other device sent something this app can't use. Try again, or use your Security Key."),
        (Some("m.invalid_message"), "The other device sent something this app can't use. Try again, or use your Security Key."),
        (Some("m.unknown_method"), "The other device sent something this app can't use. Try again, or use your Security Key."),
        (Some("org.example.custom"), "The verification was cancelled (org.example.custom)."),
        (Some(""), "The verification was cancelled."),
        (Some("  "), "The verification was cancelled."),
        (None, "The verification was cancelled."),
    ];

    #[test]
    fn cancel_note_shared_vectors() {
        for (code, want) in CANCEL {
            assert_eq!(cancel_note(*code), *want, "code {code:?}");
        }
    }

    #[test]
    fn cancel_note_never_leads_with_the_raw_code() {
        for (code, _) in CANCEL {
            let n = cancel_note(*code);
            assert!(!n.starts_with("Verification cancelled:"), "{n}");
            assert!(!n.starts_with("m."), "{n}");
        }
    }

    /// The automatic start's settle (S3) — same value as the web's SAS_AUTO_START_SETTLE_MS.
    #[test]
    fn auto_start_settle_is_300_ms() {
        assert_eq!(AUTO_START_SETTLE_MS, 300);
    }

    #[test]
    fn wait_note_exact() {
        assert_eq!(
            wait_note("Elena's iPhone"),
            "Elena's iPhone is ready. On it, choose \"Verify by emoji\" — in the old Element app, tap \"Can't scan?\". Or start from here."
        );
    }

    #[test]
    fn shared_strings_exact() {
        assert_eq!(NOTE_DONE_VERIFIER, "Verified. Your other device can read your messages now.");
        assert_eq!(NOTE_DONE_REQUESTER, "This device is verified — decrypting your messages…");
        assert_eq!(NOTE_NO_SAS, "The other device did not offer emoji verification.");
    }
}
