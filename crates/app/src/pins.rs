//! pins — QA W1 (2026-11-07): a deleted (redacted) message is not a pin. The rules the composer's "📌 N" button, its
//! Pinned messages list, the bubble's 📌 and Delete share; a line-for-line port of the web's
//! `src/features/chat/lib/pins.ts`, both replay `testvectors/pins.json` (byte-identical in the two repos). The Pinned
//! widget keeps its contract rows (a deleted pin reads "Message deleted" there, its Unpin item is the way out for a pin
//! nobody could take back).

pub const PIN_DELETED: &str = "Message deleted";
pub const PIN_ENCRYPTED: &str = "Encrypted message — open the room around it to read it";

/// A pinned row's line.
pub fn pin_row_body(encrypted: bool, redacted: bool, text: &str) -> String {
    // deleted first: a redaction keeps an encrypted event's m.room.encrypted type and drops its content — it is gone,
    // not waiting for a key
    if redacted {
        PIN_DELETED.to_string()
    } else if encrypted {
        PIN_ENCRYPTED.to_string()
    } else {
        text.to_string()
    }
}

/// The pins the 📌 N button counts and its list shows, in the given order: a message known to be deleted is not one
/// (a pin someone else's Delete left behind, or one I may not take back). The app applies it in ConversationView.qml
/// (rows of pinnedJson with `"deleted": true` are left out); this is the rule's reference, replayed by the vectors.
#[cfg_attr(not(test), allow(dead_code))]
pub fn shown_pins(pinned: &[String], deleted: &std::collections::HashSet<String>) -> Vec<String> {
    pinned.iter().filter(|id| !deleted.contains(*id)).cloned().collect()
}

/// Deleting `id` also unpins it: the room's new pin list to write (every copy of the id dropped), or None = no write
/// (it isn't pinned, or I may not edit the room's pins — then it is only hidden, [`shown_pins`]).
pub fn pins_after_delete(pinned: &[String], id: &str, may_pin: bool) -> Option<Vec<String>> {
    if !may_pin || !pinned.iter().any(|p| p == id) {
        return None;
    }
    Some(pinned.iter().filter(|p| *p != id).cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const VECTORS: &str = include_str!("testvectors/pins.json");

    fn strs(v: &Value) -> Vec<String> {
        v.as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn row_body_deleted_wins_over_encrypted() {
        let v: Value = serde_json::from_str(VECTORS).unwrap();
        for c in v["rowBody"].as_array().unwrap() {
            let got = pin_row_body(c["encrypted"].as_bool().unwrap(), c["redacted"].as_bool().unwrap(), c["text"].as_str().unwrap());
            assert_eq!(got, c["want"].as_str().unwrap(), "{}", c["why"]);
        }
    }

    #[test]
    fn a_deleted_message_neither_counts_nor_lists() {
        let v: Value = serde_json::from_str(VECTORS).unwrap();
        for c in v["shown"].as_array().unwrap() {
            let deleted = strs(&c["deleted"]).into_iter().collect();
            assert_eq!(shown_pins(&strs(&c["pinned"]), &deleted), strs(&c["want"]), "{}", c["why"]);
        }
    }

    #[test]
    fn deleting_a_pinned_message_unpins_it_when_i_may() {
        let v: Value = serde_json::from_str(VECTORS).unwrap();
        for c in v["afterDelete"].as_array().unwrap() {
            let want = if c["want"].is_null() { None } else { Some(strs(&c["want"])) };
            let got = pins_after_delete(&strs(&c["pinned"]), c["id"].as_str().unwrap(), c["mayPin"].as_bool().unwrap());
            assert_eq!(got, want, "{}", c["why"]);
        }
    }

    /// The byte copy in Tin Hat Web equals ours when a web checkout is named by `TINHAT_WEB_DIR` (skipped when unset;
    /// a named checkout without the file fails).
    #[test]
    fn vectors_are_the_webs() {
        let Some(dir) = std::env::var_os("TINHAT_WEB_DIR") else { return };
        let path = std::path::Path::new(&dir).join("src/features/chat/lib/testvectors/pins.json");
        let theirs = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(VECTORS, theirs);
    }
}
