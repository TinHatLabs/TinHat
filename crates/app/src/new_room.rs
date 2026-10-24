//! new_room.rs — what a room made with "New room" starts with (2026-10-23: rooms are encrypted from the start; only the
//! guests' rooms aren't — then option (a): a PUBLIC room inside a space stays unencrypted, so people who join later
//! can read what was said before). Shared logic: Tin
//! Hat Web's `src/features/nav/newRoom.ts` builds the same list and shows the same line, and both replay the contract
//! vectors `testvectors/new-room/vectors.json` (reference: contract `tools/new-room-vectors.py`; byte copy in
//! `src/testvectors/new-room.json`).
//!
//!   a space                      → []  (holds no messages; stays unencrypted, as before)
//!   a PUBLIC room inside a space → [join rule "restricted" to the space's members] — NOT encrypted: the space's
//!                                  members walk in without an invite, and end-to-end encryption gives such a joiner no
//!                                  earlier messages (only an invite carries them)
//!   any other room               → [encryption]  (private, in a space or not; public outside a space)
//!
//! The guest room is the server's (docs/GUESTS.md in the server repo) and stays unencrypted; the apps never make it.
use serde_json::{json, Value};

/// m.room.encryption for every encrypted room the app makes: the spec's defaults — the same content matrix-sdk's
/// create_dm gives a DM (`RoomEncryptionEventContent::with_recommended_defaults`).
pub fn room_encryption() -> Value {
    json!({ "algorithm": "m.megolm.v1.aes-sha2", "rotation_period_ms": 604800000u64, "rotation_period_msgs": 100u64 })
}

/// The space a room goes into: `parent`, unless it is empty (None/"" = none).
fn space_of(parent: Option<&str>) -> Option<&str> {
    parent.filter(|p| !p.is_empty())
}

/// createRoom's initial_state for New room, as whole events (type + state_key + content). `parent` = the space it goes
/// into (ignored for a space).
pub fn initial_state(is_space: bool, is_private: bool, parent: Option<&str>) -> Vec<Value> {
    if is_space {
        return Vec::new();
    }
    // A PUBLIC room in a space: the space's members walk in without an invite (the Discord behaviour), unencrypted.
    if let (Some(space), false) = (space_of(parent), is_private) {
        return vec![json!({
            "type": "m.room.join_rules",
            "state_key": "",
            "content": { "join_rule": "restricted", "allow": [{ "type": "m.room_membership", "room_id": space }] },
        })];
    }
    // A PRIVATE room stays invite-only even inside a space (the preset) — joining a space is never a key to its private
    // rooms — and is encrypted; so is a public room outside a space.
    vec![json!({ "type": "m.room.encryption", "state_key": "", "content": room_encryption() })]
}

/// The line New room shows under its choices — the web's NEW_ROOM_HINT, word for word (shared vectors).
pub const HINT_PRIVATE: &str = "End-to-end encrypted: only the people in it can read the messages.";
pub const HINT_PUBLIC: &str = "End-to-end encrypted: people who join without an invite can't read earlier messages.";
pub const HINT_OPEN: &str = "Not end-to-end encrypted, so people who join later can read earlier messages, and so can the server.";

/// The web's newRoomHint: "" = no line (a space).
pub fn hint(is_space: bool, is_private: bool, parent: Option<&str>) -> &'static str {
    if is_space {
        ""
    } else if space_of(parent).is_some() && !is_private {
        HINT_OPEN
    } else if is_private {
        HINT_PRIVATE
    } else {
        HINT_PUBLIC
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cases() -> Vec<Value> {
        let v: Value = serde_json::from_str(include_str!("testvectors/new-room.json")).unwrap();
        v["cases"].as_array().unwrap().clone()
    }
    fn input(c: &Value) -> (bool, bool, Option<&str>) {
        let i = &c["input"];
        (i["space"].as_bool().unwrap(), i["private"].as_bool().unwrap(), i["parent"].as_str())
    }

    #[test]
    fn new_room_shared_vectors_initial_state() {
        let cases = cases();
        assert!(cases.len() >= 9);
        for c in &cases {
            let (s, p, par) = input(c);
            assert_eq!(Value::Array(initial_state(s, p, par)), c["want"], "{}", c["name"]);
        }
    }

    #[test]
    fn new_room_shared_vectors_the_line_new_room_shows() {
        for c in &cases() {
            let (s, p, par) = input(c);
            assert_eq!(hint(s, p, par), c["hint"].as_str().unwrap(), "{}", c["name"]);
        }
    }

    /// The byte copy equals the shared contract's vectors when a contract checkout is named by
    /// `TINHAT_CONTRACT_DIR` (skipped when it is unset or the file is absent), as dm_target.rs.
    #[test]
    fn vectors_are_the_contracts() {
        let Some(dir) = std::env::var_os("TINHAT_CONTRACT_DIR") else { return };
        let path = std::path::Path::new(&dir).join("testvectors/new-room/vectors.json");
        if let Ok(theirs) = std::fs::read_to_string(path) {
            assert_eq!(include_str!("testvectors/new-room.json"), theirs);
        }
    }

    fn encrypted(is_space: bool, is_private: bool, parent: Option<&str>) -> usize {
        initial_state(is_space, is_private, parent).iter().filter(|e| e["type"] == "m.room.encryption").count()
    }

    #[test]
    fn option_a_only_a_public_room_in_a_space_is_unencrypted_no_space_is_encrypted() {
        for is_private in [true, false] {
            for parent in [None, Some(""), Some("!s:example.org")] {
                let open = !is_private && space_of(parent).is_some();
                assert_eq!(encrypted(false, is_private, parent), if open { 0 } else { 1 }, "{is_private} {parent:?}");
                let want = if open { HINT_OPEN } else if is_private { HINT_PRIVATE } else { HINT_PUBLIC };
                assert_eq!(hint(false, is_private, parent), want);
                assert!(initial_state(true, is_private, parent).is_empty());
                assert_eq!(hint(true, is_private, parent), "");
            }
        }
    }

    #[test]
    fn the_line_says_encrypted_exactly_when_the_room_is() {
        for c in &cases() {
            let (s, p, par) = input(c);
            if s {
                continue;
            }
            assert_eq!(c["hint"].as_str().unwrap().starts_with("End-to-end encrypted"), encrypted(s, p, par) == 1, "{}", c["name"]);
        }
    }

    #[test]
    fn the_content_is_the_dms() {
        // matrix-sdk's create_dm content, so a room and a DM start the same way
        use matrix_sdk::ruma::events::room::encryption::RoomEncryptionEventContent;
        let dm = serde_json::to_value(RoomEncryptionEventContent::with_recommended_defaults()).unwrap();
        assert_eq!(dm, room_encryption());
    }

    #[test]
    fn every_entry_is_a_valid_initial_state_event() {
        // what do_create_room hands matrix-sdk: each entry must parse as an initial-state event of its type
        use matrix_sdk::ruma::events::AnyInitialStateEvent;
        for (is_private, parent) in [(false, Some("!s:example.org")), (true, Some("!s:example.org")), (false, None)] {
            for e in initial_state(false, is_private, parent) {
                let parsed: AnyInitialStateEvent = serde_json::from_value(e.clone()).unwrap_or_else(|err| panic!("{e}: {err}"));
                assert_eq!(parsed.event_type().to_string(), e["type"].as_str().unwrap());
            }
        }
    }
}
