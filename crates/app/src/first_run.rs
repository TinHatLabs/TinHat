//! L4 first-run rules (Sunday QoL, TRIAGE D09.3 + A01.0/P4). SHARED LOGIC: Tin Hat Web's src/features/nav/firstrun.ts
//! applies the same rules; both replay `first-run.json` (byte copy here in `src/testvectors/first-run.json`, on the web in
//! `src/features/nav/testvectors/first-run.json`). The guest room's confirm words live in guests.rs (`guest_confirm`).
//!
//!   dm_waiting_line   a DM whose other person hasn't accepted yet: "Waiting for Sam to join" (else "")
//!   no_rooms_plan     no room open and none in the list: "No rooms yet" + New DM / New room (or the invites hint)
use serde_json::{json, Value};

/// Under "Waiting for … to join" in an empty DM.
pub const DM_WAITING_EXPLAIN: &str = "They'll see your messages once they join.";

/// `dm` = the room is a DM; `peer` = its one other person ("" = none); `membership` = theirs in the room; `name` = their
/// display name (blank → the user id). The line only while the other person is still invited.
pub fn dm_waiting_line(dm: bool, peer: &str, membership: &str, name: &str) -> String {
    if !dm || peer.is_empty() || membership != "invite" {
        return String::new();
    }
    let n = name.trim();
    format!("Waiting for {} to join", if n.is_empty() { peer } else { n })
}

/// D09.4: a DM invite's tooltip names who is really asking — the display name AND the full address (a display name alone
/// can copy anyone's): "Sam (@sam:example.com) wants to chat — click to look".
pub fn dm_invite_tip(name: &str, mxid: &str) -> String {
    let (n, m) = (name.trim(), mxid.trim());
    let who = if !m.is_empty() && !n.is_empty() && n != m {
        format!("{n} ({m})")
    } else if !n.is_empty() {
        n.to_string()
    } else if !m.is_empty() {
        m.to_string()
    } else {
        "Someone".to_string()
    };
    format!("{who} wants to chat — click to look")
}

/// The timeline island before any room is open, when the account has no rooms (spaces don't count): Null = there are
/// rooms (the usual "Pick a room to start"), else `{title, explanation, buttons: ["newDm","newRoom"] | []}`.
pub fn no_rooms_plan(rooms: i64, invites: i64) -> Value {
    if rooms > 0 {
        return Value::Null;
    }
    if invites > 0 {
        return json!({ "title": "You've been invited", "explanation": "Your invites are in your room list. Accept one to start chatting.", "buttons": [] });
    }
    json!({ "title": "No rooms yet", "explanation": "Start a chat with a friend, or make a room for your group.", "buttons": ["newDm", "newRoom"] })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vectors() -> Value {
        serde_json::from_str(include_str!("testvectors/first-run.json")).unwrap()
    }

    #[test]
    fn dm_waiting_shared_vectors() {
        let v = vectors();
        assert_eq!(v["dmWaitingExplain"].as_str().unwrap(), DM_WAITING_EXPLAIN);
        let cases = v["dmWaiting"].as_array().unwrap();
        assert!(cases.len() >= 8);
        for c in cases {
            let got = dm_waiting_line(
                c["dm"].as_bool().unwrap(),
                c["peer"].as_str().unwrap(),
                c["membership"].as_str().unwrap(),
                c["name"].as_str().unwrap(),
            );
            assert_eq!(got, c["want"].as_str().unwrap(), "{c}");
        }
    }

    #[test]
    fn dm_invite_tip_shared_vectors() {
        let v = vectors();
        let cases = v["dmInviteTip"].as_array().unwrap();
        assert!(cases.len() >= 4);
        for c in cases {
            assert_eq!(dm_invite_tip(c["name"].as_str().unwrap(), c["mxid"].as_str().unwrap()), c["want"].as_str().unwrap(), "{c}");
        }
    }

    #[test]
    fn no_rooms_shared_vectors() {
        let v = vectors();
        let cases = v["noRooms"].as_array().unwrap();
        assert!(cases.len() >= 4);
        for c in cases {
            assert_eq!(no_rooms_plan(c["rooms"].as_i64().unwrap(), c["invites"].as_i64().unwrap()), c["want"], "{c}");
        }
    }

    #[test]
    fn guest_confirm_shared_vectors() {
        let v = vectors();
        let cases = v["guestConfirm"].as_array().unwrap();
        assert!(cases.len() >= 10);
        for c in cases {
            let got = crate::guests::guest_confirm(
                c["what"].as_str().unwrap(),
                c["guests"].as_i64().unwrap(),
                c["waiting"].as_u64().unwrap() as usize,
            );
            assert_eq!(got, c["want"], "{c}");
        }
    }
}
