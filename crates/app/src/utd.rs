//! utd.rs — the words for a message this device can't read (selfhost/firstrun §3). SHARED LOGIC + WORDING with the
//! web's src/features/chat/lib/utd.ts: the same causes, the same sentences, and utd.test.ts's table replayed below.
//!
//! The old line said "Can't read this message — …" for every such message, which hints that verifying would help —
//! wrong when the message is older than the person's first sign-in (the Tin Hat Server blind tests: a friend wrote in
//! General before the owner had ever opened Tin Hat; no device of theirs existed, so nobody can ever unlock it).
//!
//! matrix-sdk says why a decryption failed (`UnableToDecryptReason`, kept on the event's JSON as `tinhat_utd` by the
//! window fetch and the re-decrypt pass). With this device's creation time, the key backup's state and the event's
//! `unsigned.membership`, that becomes the web's failure code (matrix-js-sdk's DecryptionFailureCode names, the input
//! of the web's utdCause), then the cause and its sentence:
//!
//!   missing key / unknown index, not a member when it was sent        HISTORICAL_MESSAGE_USER_NOT_JOINED → beforeJoined
//!   missing key / unknown index, sent before this device existed      HISTORICAL_MESSAGE_{NO_KEY_BACKUP, BACKUP_UNCONFIGURED,
//!                                                                     WORKING_BACKUP} → beforeFirstSignIn on the account's
//!                                                                     first device, beforeThisDeviceVerify when a backup
//!                                                                     exists that this unverified device can't open, else
//!                                                                     beforeThisDevice
//!   withheld "m.unverified"                                           → withheldUnverified
//!   withheld otherwise                                                → withheld
//!   anything else (the key may still arrive)                          → waiting
use matrix_sdk::deserialized_responses::UnableToDecryptReason;

/// The JSON key the reason travels under on a window event (private to this app; the window cache may keep it).
pub const KEY: &str = "tinhat_utd";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UtdCause {
    BeforeFirstSignIn,
    BeforeThisDevice,
    BeforeThisDeviceVerify,
    BeforeJoined,
    WithheldUnverified,
    Withheld,
    Waiting,
}

impl UtdCause {
    pub const ALL: [UtdCause; 7] = [
        UtdCause::BeforeFirstSignIn,
        UtdCause::BeforeThisDevice,
        UtdCause::BeforeThisDeviceVerify,
        UtdCause::BeforeJoined,
        UtdCause::WithheldUnverified,
        UtdCause::Withheld,
        UtdCause::Waiting,
    ];

    /// The web's UtdCause string.
    pub fn key(self) -> &'static str {
        match self {
            UtdCause::BeforeFirstSignIn => "beforeFirstSignIn",
            UtdCause::BeforeThisDevice => "beforeThisDevice",
            UtdCause::BeforeThisDeviceVerify => "beforeThisDeviceVerify",
            UtdCause::BeforeJoined => "beforeJoined",
            UtdCause::WithheldUnverified => "withheldUnverified",
            UtdCause::Withheld => "withheld",
            UtdCause::Waiting => "waiting",
        }
    }

    /// The web's UTD_TEXT, word for word.
    pub fn text(self) -> &'static str {
        match self {
            UtdCause::BeforeFirstSignIn => "Sent before you first signed in, so it can't be unlocked.",
            UtdCause::BeforeThisDevice => {
                "Sent before you signed in on this device. Messages from before your first sign-in can't be unlocked."
            }
            UtdCause::BeforeThisDeviceVerify => "Sent before you signed in on this device. Verify this device to unlock it.",
            UtdCause::BeforeJoined => "Sent while you weren't in this room, so it can't be unlocked.",
            UtdCause::WithheldUnverified => {
                "The sender only shares keys with verified devices. Verify this device to read what they send next."
            }
            UtdCause::Withheld => "The sender's app didn't share the key to this message.",
            UtdCause::Waiting => "Can't read this message yet — its key hasn't reached this device.",
        }
    }
}

const BEFORE_THIS_DEVICE: [&str; 3] = [
    "HISTORICAL_MESSAGE_NO_KEY_BACKUP",
    "HISTORICAL_MESSAGE_BACKUP_UNCONFIGURED",
    "HISTORICAL_MESSAGE_WORKING_BACKUP",
];

/// The web's utdCause (utd.ts): `code` is a matrix-js-sdk DecryptionFailureCode name ("" when not known),
/// `first_device` = this device set up the account's encryption while no other device existed, `verified` = this
/// device holds the cross-signing keys.
pub fn utd_cause(code: &str, first_device: bool, verified: bool) -> UtdCause {
    if BEFORE_THIS_DEVICE.contains(&code) {
        if first_device {
            return UtdCause::BeforeFirstSignIn;
        }
        if code == "HISTORICAL_MESSAGE_BACKUP_UNCONFIGURED" && !verified {
            return UtdCause::BeforeThisDeviceVerify;
        }
        return UtdCause::BeforeThisDevice;
    }
    match code {
        "HISTORICAL_MESSAGE_USER_NOT_JOINED" => UtdCause::BeforeJoined,
        "MEGOLM_KEY_WITHHELD_FOR_UNVERIFIED_DEVICE" => UtdCause::WithheldUnverified,
        "MEGOLM_KEY_WITHHELD" => UtdCause::Withheld,
        _ => UtdCause::Waiting,
    }
}

/// What matrix-sdk said about a failure, reduced to what the cause needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    MissingKey,
    UnknownIndex,
    Withheld(String),
    Other,
}

impl Failure {
    pub fn from_reason(r: &UnableToDecryptReason) -> Self {
        match r {
            UnableToDecryptReason::MissingMegolmSession { withheld_code: Some(c) } => Failure::Withheld(AsRef::<str>::as_ref(c).to_string()),
            UnableToDecryptReason::MissingMegolmSession { withheld_code: None } => Failure::MissingKey,
            UnableToDecryptReason::UnknownMegolmMessageIndex => Failure::UnknownIndex,
            _ => Failure::Other,
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Failure::MissingKey => serde_json::json!({ "r": "missing" }),
            Failure::UnknownIndex => serde_json::json!({ "r": "index" }),
            Failure::Withheld(c) => serde_json::json!({ "r": "withheld", "w": c }),
            Failure::Other => serde_json::json!({ "r": "other" }),
        }
    }

    pub fn from_json(v: Option<&serde_json::Value>) -> Option<Self> {
        let v = v?;
        Some(match v.get("r").and_then(|r| r.as_str())? {
            "missing" => Failure::MissingKey,
            "index" => Failure::UnknownIndex,
            "withheld" => Failure::Withheld(v.get("w").and_then(|w| w.as_str()).unwrap_or("").to_string()),
            _ => Failure::Other,
        })
    }
}

/// Keep matrix-sdk's reason on a window event (`ev` = the event's JSON), for the row it becomes.
pub fn annotate(ev: &mut serde_json::Value, reason: &UnableToDecryptReason) {
    if let Some(o) = ev.as_object_mut() {
        o.insert(KEY.to_string(), Failure::from_reason(reason).to_json());
    }
}

/// This device and account, read once per window build.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Context {
    /// when this device was created (ms since the epoch; 0 = unknown)
    pub device_created_ms: u64,
    /// a key backup exists on the server
    pub backup_exists: bool,
    /// this device has the backup's key (it can open the backup)
    pub backup_enabled_here: bool,
    /// this device holds the cross-signing keys
    pub verified: bool,
    /// this device set up the account's encryption while no other device existed (the `first-device` marker)
    pub first_device: bool,
}

/// matrix-js-sdk's DecryptionFailureCode for a failure (None = not known: treated as a missing key, the usual case),
/// given the event's `unsigned.membership` and `origin_server_ts`.
pub fn failure_code(f: Option<&Failure>, membership: Option<&str>, sent_ms: u64, ctx: &Context) -> &'static str {
    match f {
        Some(Failure::Withheld(c)) if c == "m.unverified" => "MEGOLM_KEY_WITHHELD_FOR_UNVERIFIED_DEVICE",
        Some(Failure::Withheld(_)) => "MEGOLM_KEY_WITHHELD",
        Some(Failure::Other) => "UNKNOWN_ERROR",
        Some(Failure::MissingKey) | Some(Failure::UnknownIndex) | None => {
            if let Some(m) = membership.filter(|m| !m.is_empty()) {
                if m != "join" && m != "invite" {
                    return "HISTORICAL_MESSAGE_USER_NOT_JOINED";
                }
            }
            if sent_ms > 0 && ctx.device_created_ms > 0 && sent_ms < ctx.device_created_ms {
                return if !ctx.backup_exists {
                    "HISTORICAL_MESSAGE_NO_KEY_BACKUP"
                } else if !ctx.backup_enabled_here {
                    "HISTORICAL_MESSAGE_BACKUP_UNCONFIGURED"
                } else {
                    "HISTORICAL_MESSAGE_WORKING_BACKUP"
                };
            }
            if matches!(f, Some(Failure::UnknownIndex)) {
                "OLM_UNKNOWN_MESSAGE_INDEX"
            } else {
                "MEGOLM_UNKNOWN_INBOUND_SESSION_ID"
            }
        }
    }
}

/// The cause for an undecryptable window event (its JSON) on this device.
pub fn cause_of(ev: &serde_json::Value, ctx: &Context) -> UtdCause {
    let failure = Failure::from_json(ev.get(KEY));
    let membership = ev.get("unsigned").and_then(|u| u.get("membership")).and_then(|m| m.as_str());
    let sent = ev.get("origin_server_ts").and_then(|t| t.as_u64()).unwrap_or(0);
    utd_cause(failure_code(failure.as_ref(), membership, sent, ctx), ctx.first_device, ctx.verified)
}

// ---- which of them are rows (2026-10-23: rooms are encrypted by default) ----
// The web's utdRow (utd.ts), replayed by utd.test.ts's table below. An event this device can't read still shows its
// relation: `m.relates_to` stays OUTSIDE the ciphertext. So a deleted one is a deleted message (nothing is left to
// unlock), an edit / poll vote or end (or in-room verification step) / reaction is never a bubble of its own, and
// anything else is a locked bubble with the cause's sentence.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UtdRow {
    Row,
    Deleted,
    Hidden,
}

impl UtdRow {
    /// The web's UtdRow string.
    pub fn key(self) -> &'static str {
        match self {
            UtdRow::Row => "row",
            UtdRow::Deleted => "deleted",
            UtdRow::Hidden => "hidden",
        }
    }
}

pub const HIDDEN_RELATIONS: [&str; 3] = ["m.replace", "m.reference", "m.annotation"];

/// The web's utdRow: `redacted` = the event was deleted, `rel_type` = its cleartext relation (None = none).
pub fn utd_row(redacted: bool, rel_type: Option<&str>) -> UtdRow {
    if redacted {
        return UtdRow::Deleted;
    }
    match rel_type {
        Some(r) if HIDDEN_RELATIONS.contains(&r) => UtdRow::Hidden,
        _ => UtdRow::Row,
    }
}

/// utd_row for an undecryptable window event (its JSON: `unsigned.redacted_because`, `content.m.relates_to`).
pub fn utd_row_of(ev: &serde_json::Value) -> UtdRow {
    let redacted = ev.get("unsigned").and_then(|u| u.get("redacted_because")).is_some();
    let rel = ev.get("content").and_then(|c| c.get("m.relates_to")).and_then(|r| r.get("rel_type")).and_then(|t| t.as_str());
    utd_row(redacted, rel)
}

/// The `first-device` marker's value for this session ("<user>|<device>": a later sign-in has another device id).
pub fn first_device_marker(user: &str, device: &str) -> String {
    format!("{user}|{device}")
}

/// Read this device's context (store reads; the backup's existence is matrix-sdk's cached answer, fetched once).
pub async fn context(client: &matrix_sdk::Client) -> Context {
    let enc = client.encryption();
    let device_created_ms = u64::from(enc.device_creation_timestamp().await.get());
    let backups = enc.backups();
    let backup_enabled_here = backups.are_enabled().await;
    let backup_exists = backup_enabled_here || backups.exists_on_server().await.unwrap_or(false);
    let verified = match enc.cross_signing_status().await {
        Some(s) => s.has_master && s.has_self_signing && s.has_user_signing,
        None => false,
    };
    let first_device = match (client.user_id(), client.device_id()) {
        (Some(u), Some(d)) => crate::settings::overlay_pref_read("ui-first-device")
            .is_some_and(|m| m.trim() == first_device_marker(u.as_str(), d.as_str())),
        _ => false,
    };
    Context { device_created_ms, backup_exists, backup_enabled_here, verified, first_device }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: [&str; 3] = BEFORE_THIS_DEVICE;

    // ---- the web's utd.test.ts, replayed ----

    #[test]
    fn the_tasks_sentence_for_a_message_from_before_this_device() {
        assert_eq!(
            UtdCause::BeforeThisDevice.text(),
            "Sent before you signed in on this device. Messages from before your first sign-in can't be unlocked."
        );
    }

    #[test]
    fn before_the_accounts_first_device_say_so_never_suggest_verifying() {
        for code in H {
            for verified in [true, false] {
                assert_eq!(utd_cause(code, true, verified), UtdCause::BeforeFirstSignIn, "{code} verified={verified}");
                assert!(!utd_cause(code, true, verified).text().to_lowercase().contains("verify"), "{code}");
            }
        }
    }

    #[test]
    fn before_this_device_not_the_first_one() {
        assert_eq!(utd_cause("HISTORICAL_MESSAGE_WORKING_BACKUP", false, true), UtdCause::BeforeThisDevice);
        assert_eq!(utd_cause("HISTORICAL_MESSAGE_WORKING_BACKUP", false, false), UtdCause::BeforeThisDevice);
        assert_eq!(utd_cause("HISTORICAL_MESSAGE_NO_KEY_BACKUP", false, false), UtdCause::BeforeThisDevice, "no backup");
        assert_eq!(utd_cause("HISTORICAL_MESSAGE_BACKUP_UNCONFIGURED", false, false), UtdCause::BeforeThisDeviceVerify);
        assert_eq!(utd_cause("HISTORICAL_MESSAGE_BACKUP_UNCONFIGURED", false, true), UtdCause::BeforeThisDevice);
    }

    #[test]
    fn the_other_causes() {
        let f = |c: &str| utd_cause(c, false, true);
        assert_eq!(f("HISTORICAL_MESSAGE_USER_NOT_JOINED"), UtdCause::BeforeJoined);
        assert_eq!(utd_cause("HISTORICAL_MESSAGE_USER_NOT_JOINED", true, true), UtdCause::BeforeJoined, "not a member beats first device");
        assert_eq!(f("MEGOLM_KEY_WITHHELD_FOR_UNVERIFIED_DEVICE"), UtdCause::WithheldUnverified);
        assert_eq!(f("MEGOLM_KEY_WITHHELD"), UtdCause::Withheld);
        for c in [
            "MEGOLM_UNKNOWN_INBOUND_SESSION_ID",
            "OLM_UNKNOWN_MESSAGE_INDEX",
            "UNKNOWN_ERROR",
            "SENDER_IDENTITY_PREVIOUSLY_VERIFIED",
            "UNSIGNED_SENDER_DEVICE",
            "UNKNOWN_SENDER_DEVICE",
            "",
            "SOMETHING_NEW",
        ] {
            assert_eq!(f(c), UtdCause::Waiting, "{c}");
        }
    }

    #[test]
    fn plain_words_no_jargon_every_sentence_ends_with_a_full_stop() {
        for c in UtdCause::ALL {
            let t = c.text();
            assert!(t.ends_with('.'), "{}", c.key());
            for w in ["megolm", "olm", "session", "backup", "cross-sign", "decrypt", "e2ee", "key backup"] {
                assert!(!t.to_lowercase().contains(w), "{}: {w}", c.key());
            }
        }
    }

    // 2026-10-23: which unreadable events are rows — utd.test.ts's ROWS table, word for word
    #[test]
    fn which_unreadable_events_are_rows_deleted_hidden_relations_locked_bubbles() {
        let rows: [(bool, Option<&str>, &str); 9] = [
            (false, None, "row"),
            (false, Some("m.thread"), "row"),
            (false, Some("m.replace"), "hidden"),
            (false, Some("m.reference"), "hidden"),
            (false, Some("m.annotation"), "hidden"),
            (false, Some(""), "row"),
            (false, Some("m.something_new"), "row"),
            (true, None, "deleted"),
            (true, Some("m.replace"), "deleted"),
        ];
        for (redacted, rel, want) in rows {
            assert_eq!(utd_row(redacted, rel).key(), want, "{redacted} {rel:?}");
        }
    }

    #[test]
    fn a_window_events_row() {
        let ev = |unsigned: serde_json::Value, content: serde_json::Value| {
            serde_json::json!({ "type": "m.room.encrypted", "unsigned": unsigned, "content": content })
        };
        let edit = serde_json::json!({ "algorithm": "m.megolm.v1.aes-sha2", "ciphertext": "x",
            "m.relates_to": { "rel_type": "m.replace", "event_id": "$t" } });
        let vote = serde_json::json!({ "ciphertext": "x", "m.relates_to": { "rel_type": "m.reference", "event_id": "$p" } });
        let reply = serde_json::json!({ "ciphertext": "x", "m.relates_to": { "m.in_reply_to": { "event_id": "$p" } } });
        assert_eq!(utd_row_of(&ev(serde_json::json!({}), edit)), UtdRow::Hidden);
        assert_eq!(utd_row_of(&ev(serde_json::json!({}), vote)), UtdRow::Hidden);
        assert_eq!(utd_row_of(&ev(serde_json::json!({}), reply)), UtdRow::Row, "a reply is a message of its own");
        assert_eq!(utd_row_of(&ev(serde_json::json!({}), serde_json::json!({ "ciphertext": "x" }))), UtdRow::Row);
        assert_eq!(
            utd_row_of(&ev(serde_json::json!({ "redacted_because": { "type": "m.room.redaction" } }), serde_json::json!({}))),
            UtdRow::Deleted
        );
    }

    // ---- the desktop's own step: matrix-sdk's reason → the web's code ----

    const DEVICE: u64 = 1_700_000_000_000;
    fn ctx(backup_exists: bool, backup_enabled_here: bool) -> Context {
        Context { device_created_ms: DEVICE, backup_exists, backup_enabled_here, verified: false, first_device: false }
    }

    #[test]
    fn withheld_keys() {
        let c = ctx(true, true);
        let w = |code: &str| Failure::Withheld(code.to_string());
        assert_eq!(failure_code(Some(&w("m.unverified")), None, DEVICE - 1, &c), "MEGOLM_KEY_WITHHELD_FOR_UNVERIFIED_DEVICE");
        for code in ["m.blacklisted", "m.unauthorised", "m.unavailable", "m.no_olm", "org.example.custom"] {
            assert_eq!(failure_code(Some(&w(code)), Some("leave"), DEVICE - 1, &c), "MEGOLM_KEY_WITHHELD", "{code}");
        }
    }

    #[test]
    fn not_a_member_when_it_was_sent() {
        for m in ["leave", "ban", "knock"] {
            assert_eq!(failure_code(Some(&Failure::MissingKey), Some(m), DEVICE + 1, &ctx(true, true)), "HISTORICAL_MESSAGE_USER_NOT_JOINED", "{m}");
            assert_eq!(failure_code(None, Some(m), DEVICE - 1, &ctx(false, false)), "HISTORICAL_MESSAGE_USER_NOT_JOINED", "{m}");
        }
        for m in ["join", "invite", ""] {
            assert_eq!(failure_code(Some(&Failure::MissingKey), Some(m), DEVICE + 1, &ctx(true, true)), "MEGOLM_UNKNOWN_INBOUND_SESSION_ID", "{m}");
        }
    }

    #[test]
    fn sent_before_this_device() {
        let before = DEVICE - 60_000;
        assert_eq!(failure_code(Some(&Failure::MissingKey), None, before, &ctx(false, false)), "HISTORICAL_MESSAGE_NO_KEY_BACKUP");
        assert_eq!(failure_code(Some(&Failure::MissingKey), None, before, &ctx(true, false)), "HISTORICAL_MESSAGE_BACKUP_UNCONFIGURED");
        assert_eq!(failure_code(Some(&Failure::UnknownIndex), None, before, &ctx(true, true)), "HISTORICAL_MESSAGE_WORKING_BACKUP");
        assert_eq!(failure_code(None, Some("join"), before, &ctx(true, true)), "HISTORICAL_MESSAGE_WORKING_BACKUP");
        // after this device existed: the key may still arrive
        assert_eq!(failure_code(Some(&Failure::MissingKey), None, DEVICE + 1, &ctx(false, false)), "MEGOLM_UNKNOWN_INBOUND_SESSION_ID");
        assert_eq!(failure_code(Some(&Failure::UnknownIndex), None, DEVICE + 1, &ctx(true, true)), "OLM_UNKNOWN_MESSAGE_INDEX");
        // unknown times never guess "historical"
        assert_eq!(failure_code(None, None, 0, &ctx(false, false)), "MEGOLM_UNKNOWN_INBOUND_SESSION_ID");
        assert_eq!(failure_code(None, None, before, &Context::default()), "MEGOLM_UNKNOWN_INBOUND_SESSION_ID");
        assert_eq!(failure_code(Some(&Failure::Other), None, before, &ctx(false, false)), "UNKNOWN_ERROR");
    }

    #[test]
    fn a_window_event_end_to_end() {
        let ev = |extra: serde_json::Value| {
            let mut v = serde_json::json!({ "type": "m.room.encrypted", "origin_server_ts": DEVICE - 5_000 });
            for (k, x) in extra.as_object().unwrap() {
                v[k] = x.clone();
            }
            v
        };
        let first = Context { first_device: true, ..ctx(false, false) };
        assert_eq!(cause_of(&ev(serde_json::json!({})), &first), UtdCause::BeforeFirstSignIn);
        let mut annotated = ev(serde_json::json!({}));
        annotate(&mut annotated, &UnableToDecryptReason::MissingMegolmSession { withheld_code: None });
        assert_eq!(cause_of(&annotated, &ctx(true, false)), UtdCause::BeforeThisDeviceVerify);
        assert_eq!(cause_of(&annotated, &Context { verified: true, ..ctx(true, false) }), UtdCause::BeforeThisDevice);
        assert_eq!(cause_of(&ev(serde_json::json!({ "unsigned": { "membership": "leave" } })), &first), UtdCause::BeforeJoined);
        let withheld = ev(serde_json::json!({ KEY: { "r": "withheld", "w": "m.unverified" } }));
        assert_eq!(cause_of(&withheld, &first), UtdCause::WithheldUnverified);
        let later = serde_json::json!({ "type": "m.room.encrypted", "origin_server_ts": DEVICE + 5_000 });
        assert_eq!(cause_of(&later, &first), UtdCause::Waiting);
    }

    #[test]
    fn the_reason_round_trips_through_the_events_json() {
        for f in [Failure::MissingKey, Failure::UnknownIndex, Failure::Withheld("m.unverified".into()), Failure::Other] {
            assert_eq!(Failure::from_json(Some(&f.to_json())), Some(f.clone()));
        }
        assert_eq!(Failure::from_json(None), None);
        assert_eq!(Failure::from_json(Some(&serde_json::json!({}))), None);
    }
}
