//! Events of a server that moved to a new name (Tin Hat Server: Maintenance → "Move to your own domain", the server's
//! move.py / move_synapse.py). The move gave every event a new id. An event's relation (edit, reaction, reply, thread,
//! poll answer) sits outside the encryption, where the server renamed it, and also inside it, where nothing can change
//! it; decryption (matrix-sdk-crypto keeps the inside copy) still names the old ids. So the server leaves a note in
//! each moved event's unsigned["dev.tinhat.moved"]:
//!   former_ids  the ids the event had before (every move so far)
//!   relates_to  for an encrypted event: its relation by the new ids
//! Events pass through `normalize` where they come in (the /messages pages, a re-decryption), so everything that reads
//! content["m.relates_to"] (the chat window, reactions, edits, replies, polls, the calendar) sees today's ids.
//! Shared rule with Tin Hat Web (src/core/matrix/moved.ts): the same cases in both test suites.
use std::collections::HashMap;

use serde_json::{Map, Value};

pub const MOVED_KEY: &str = "dev.tinhat.moved";

fn note(unsigned: Option<&Value>) -> Option<&Map<String, Value>> {
    unsigned?.get(MOVED_KEY)?.as_object()
}

/// The ids a moved event had before (every move so far), from its unsigned.
pub fn former_ids(unsigned: Option<&Value>) -> Vec<String> {
    note(unsigned)
        .and_then(|n| n.get("former_ids"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

/// An event's relation as it is now: `rel` (the decrypted content's m.relates_to), its targets taken from the note when
/// the note describes the same relation (the same rel_type, and the same key for a reaction). Anything else stays.
pub fn current_relation(rel: &Value, unsigned: Option<&Value>) -> Value {
    let Some(r) = rel.as_object() else { return rel.clone() };
    let Some(moved) = note(unsigned).and_then(|n| n.get("relates_to")).and_then(Value::as_object) else {
        return rel.clone();
    };
    if moved.get("rel_type") != r.get("rel_type") {
        return rel.clone();
    }
    if r.get("rel_type").and_then(Value::as_str) == Some("m.annotation") && moved.get("key") != r.get("key") {
        return rel.clone();
    }
    let mut out = r.clone();
    if r.get("event_id").is_some_and(Value::is_string) {
        if let Some(id) = moved.get("event_id").filter(|v| v.is_string()) {
            out.insert("event_id".to_owned(), id.clone());
        }
    }
    if let Some(reply) = r.get("m.in_reply_to").and_then(Value::as_object) {
        if let Some(id) = moved.get("m.in_reply_to").and_then(|m| m.get("event_id")).filter(|v| v.is_string()) {
            let mut nr = reply.clone();
            nr.insert("event_id".to_owned(), id.clone());
            out.insert("m.in_reply_to".to_owned(), Value::Object(nr));
        }
    }
    Value::Object(out)
}

/// The event (its JSON, as the window keeps it) with its content's relation as it is now.
pub fn normalize(ev: &mut Value) {
    let Some(rel) = ev.get("content").and_then(|c| c.get("m.relates_to")).filter(|r| r.is_object()) else { return };
    let now = current_relation(rel, ev.get("unsigned"));
    if &now != rel {
        ev["content"]["m.relates_to"] = now;
    }
}

/// Old ids → today's ids, from the events at hand (ids named inside encrypted content other than its relation).
pub fn former_id_map<'a>(events: impl IntoIterator<Item = &'a Value>) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for e in events {
        let Some(id) = e.get("event_id").and_then(Value::as_str) else { continue };
        for f in former_ids(e.get("unsigned")) {
            if f != id {
                m.insert(f, id.to_owned());
            }
        }
    }
    m
}

/// What a server's /.well-known/matrix/client says about a move.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MoveInfo {
    /// on a former name: where the server went
    pub moved_to: Option<String>,
    /// on the new address: the names it had before
    pub moved_from: Vec<String>,
}

fn host(v: &Value) -> Option<String> {
    let h = v.as_str()?.trim().to_ascii_lowercase();
    let h = h.strip_suffix('.').unwrap_or(&h).to_owned();
    let labels: Vec<&str> = h.split('.').collect();
    let ok = h.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && l.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        })
        && labels.last().is_some_and(|t| t.len() >= 2);
    ok.then_some(h)
}

/// dev.tinhat.moved_to / dev.tinhat.moved_from from a well-known answer; plain host names only.
pub fn parse_move(well_known: &Value) -> MoveInfo {
    let Some(w) = well_known.as_object() else { return MoveInfo::default() };
    MoveInfo {
        moved_to: w.get("dev.tinhat.moved_to").and_then(host),
        moved_from: w
            .get("dev.tinhat.moved_from")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(host).collect())
            .unwrap_or_default(),
    }
}

/// The account has its Security Key's storage (Secret Storage) but no encryption identity on the server: the server
/// moved to a new name (the identities were made for the old one) or someone removed theirs. No other device can vouch
/// for this one, so the unlock leads with the Security Key, which restores the message keys and makes a new identity
/// under the same key.
pub fn key_first(has_identity: bool, secret_storage: bool) -> bool {
    !has_identity && secret_storage
}

#[cfg(test)]
mod tests {
    // The cases of Tin Hat Web's src/core/matrix/moved.test.ts.
    use super::*;
    use serde_json::json;

    fn ids() -> (String, String, String, String) {
        (format!("${}", "a".repeat(43)), format!("${}", "b".repeat(43)), format!("${}", "x".repeat(43)), format!("${}", "y".repeat(43)))
    }
    fn note_with(relates_to: Option<Value>, former: Vec<&str>) -> Value {
        let mut n = json!({ "former_ids": former });
        if let Some(r) = relates_to {
            n["relates_to"] = r;
        }
        json!({ MOVED_KEY: n })
    }

    #[test]
    fn a_reaction_from_before_the_move_points_at_the_new_id() {
        let (old_a, old_b, new_a, _) = ids();
        let inside = json!({ "rel_type": "m.annotation", "event_id": old_a, "key": "👍" });
        let n = note_with(Some(json!({ "rel_type": "m.annotation", "event_id": new_a, "key": "👍" })), vec![&old_b]);
        assert_eq!(current_relation(&inside, Some(&n)), json!({ "rel_type": "m.annotation", "event_id": new_a, "key": "👍" }));
    }

    #[test]
    fn edits_threads_and_replies_too() {
        let (old_a, old_b, new_a, new_b) = ids();
        let n = note_with(Some(json!({ "rel_type": "m.replace", "event_id": new_a })), vec![&old_b]);
        assert_eq!(current_relation(&json!({ "rel_type": "m.replace", "event_id": old_a }), Some(&n))["event_id"], json!(new_a));
        let thread = json!({ "rel_type": "m.thread", "event_id": old_a, "is_falling_back": true, "m.in_reply_to": { "event_id": old_b } });
        let n = note_with(Some(json!({ "rel_type": "m.thread", "event_id": new_a, "is_falling_back": true, "m.in_reply_to": { "event_id": new_b } })), vec![&old_b]);
        assert_eq!(current_relation(&thread, Some(&n)),
            json!({ "rel_type": "m.thread", "event_id": new_a, "is_falling_back": true, "m.in_reply_to": { "event_id": new_b } }));
        let n = note_with(Some(json!({ "m.in_reply_to": { "event_id": new_a } })), vec![&old_b]);
        assert_eq!(current_relation(&json!({ "m.in_reply_to": { "event_id": old_a } }), Some(&n)), json!({ "m.in_reply_to": { "event_id": new_a } }));
    }

    #[test]
    fn only_the_same_relation() {
        let (old_a, old_b, new_a, _) = ids();
        let inside = json!({ "rel_type": "m.annotation", "event_id": old_a, "key": "👍" });
        let other_key = note_with(Some(json!({ "rel_type": "m.annotation", "event_id": new_a, "key": "🎉" })), vec![&old_b]);
        assert_eq!(current_relation(&inside, Some(&other_key)), inside);
        let other_kind = note_with(Some(json!({ "rel_type": "m.replace", "event_id": new_a })), vec![&old_b]);
        assert_eq!(current_relation(&inside, Some(&other_kind)), inside);
        let reply_note = note_with(Some(json!({ "m.in_reply_to": { "event_id": new_a } })), vec![&old_b]);
        let edit = json!({ "rel_type": "m.replace", "event_id": old_a });
        assert_eq!(current_relation(&edit, Some(&reply_note)), edit);
    }

    #[test]
    fn no_note_no_relation_odd_notes() {
        let (old_a, _, new_a, _) = ids();
        let inside = json!({ "rel_type": "m.replace", "event_id": old_a });
        assert_eq!(current_relation(&inside, Some(&json!({}))), inside);
        assert_eq!(current_relation(&inside, None), inside);
        assert_eq!(current_relation(&inside, Some(&json!({ MOVED_KEY: "junk" }))), inside);
        let bad = note_with(Some(json!({ "rel_type": "m.replace", "event_id": 5 })), vec![]);
        assert_eq!(current_relation(&inside, Some(&bad)), inside);
        let mut ev = json!({ "content": { "body": "hi" }, "unsigned": note_with(Some(json!({ "rel_type": "m.replace", "event_id": new_a })), vec![]) });
        let before = ev.clone();
        normalize(&mut ev);
        assert_eq!(ev, before);
    }

    #[test]
    fn normalize_puts_todays_relation_into_the_event() {
        let (old_a, _, new_a, _) = ids();
        let mut ev = json!({ "type": "m.room.message", "content": { "body": "hi", "m.relates_to": { "rel_type": "m.replace", "event_id": old_a } },
                             "unsigned": note_with(Some(json!({ "rel_type": "m.replace", "event_id": new_a })), vec![]) });
        normalize(&mut ev);
        assert_eq!(ev["content"]["m.relates_to"]["event_id"], json!(new_a));
    }

    #[test]
    fn old_ids_map_to_todays() {
        let (old_a, old_b, new_a, new_b) = ids();
        let z = format!("${}", "z".repeat(43));
        let q = format!("${}", "q".repeat(43));
        let evs = vec![
            json!({ "event_id": new_a, "unsigned": note_with(None, vec![&old_a]) }),
            json!({ "event_id": new_b, "unsigned": note_with(None, vec![&old_b, &z]) }),
            json!({ "event_id": q, "unsigned": {} }),
            json!({ "event_id": new_a, "unsigned": note_with(None, vec![&new_a]) }),
            json!({ "unsigned": note_with(None, vec![&old_b]) }),
        ];
        let m = former_id_map(&evs);
        let mut got: Vec<(String, String)> = m.into_iter().collect();
        got.sort();
        let mut want = vec![(old_a, new_a), (old_b, new_b.clone()), (z, new_b)];
        want.sort();
        assert_eq!(got, want);
        assert_eq!(former_ids(Some(&json!({ MOVED_KEY: { "former_ids": [ids().0, 7] } }))), vec![ids().0]);
    }

    #[test]
    fn a_servers_well_known_about_a_move() {
        assert_eq!(parse_move(&json!({ "dev.tinhat.moved_to": "Chat.Example.com." })),
            MoveInfo { moved_to: Some("chat.example.com".into()), moved_from: vec![] });
        assert_eq!(parse_move(&json!({ "dev.tinhat.moved_from": ["home.local", "x y", 7, "maplehouse.tinhat.me"] })),
            MoveInfo { moved_to: None, moved_from: vec!["home.local".into(), "maplehouse.tinhat.me".into()] });
        for bad in [json!(null), json!("x"), json!([]), json!({ "dev.tinhat.moved_to": "javascript:alert(1)" }), json!({ "dev.tinhat.moved_to": "a/b.com" }),
                    json!({ "dev.tinhat.moved_to": "chat.-x" }), json!({ "dev.tinhat.moved_to": "chat.x" })] {
            assert_eq!(parse_move(&bad), MoveInfo::default());
        }
    }

    #[test]
    fn the_security_key_first() {
        assert!(key_first(false, true));
        assert!(!key_first(true, true));
        assert!(!key_first(false, false), "a brand-new account: the first device sets it up");
    }
}
