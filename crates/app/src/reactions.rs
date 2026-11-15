//! Reactions (QOL A07.0 / A07.1 / A07.2): the one key rule and the one grouping rule for a message's reaction chips.
//! - `react_key_norm`: a key's grouping form — the U+FE0E / U+FE0F variation selectors dropped, so ❤ and ❤️ are one chip
//!   and the quick ❤️ button finds yours.
//! - `group_chips`: a message's reactions → its chips, in order of first appearance (stable across reloads: the map
//!   used to be a HashMap, whose order changes per load), key = the first one sent, count = distinct SENDERS (two people
//!   with the same display name used to merge into one), mine = I am one of them.
//! SHARED LOGIC: = Tin Hat Web's features/chat/lib/reactions.ts; both replay src/testvectors/reactions.json.

/// The grouping form of a reaction key.
pub fn react_key_norm(k: &str) -> String {
    k.chars().filter(|c| *c != '\u{FE0E}' && *c != '\u{FE0F}').collect()
}

/// One reaction event of a message, as the timeline scan sees it.
#[derive(Clone, Debug, Default)]
pub struct ReactIn {
    pub key: String,
    pub sender: String,
    /// the reaction's own event id (mine: what unreact redacts)
    pub event_id: String,
    /// the sender's display name (the chip's "who" tooltip)
    pub name: String,
}

/// A message's chips from its reactions in TIMELINE order (oldest first): JSON objects
/// `{ key, count, mine, reactionId, who }` (who = display names, newest first, the web's order).
pub fn group_chips(oldest_first: &[ReactIn], me: &str) -> Vec<serde_json::Value> {
    struct G {
        key: String,
        senders: Vec<String>,
        who: Vec<String>,
        mine: bool,
        rid: String,
        mine_ids: Vec<String>,
    }
    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, G> = std::collections::HashMap::new();
    for r in oldest_first {
        if r.key.is_empty() {
            continue;
        }
        let n = react_key_norm(&r.key);
        let g = groups.entry(n.clone()).or_insert_with(|| {
            order.push(n.clone());
            G { key: r.key.clone(), senders: Vec::new(), who: Vec::new(), mine: false, rid: String::new(), mine_ids: Vec::new() }
        });
        // one person counts once (a duplicate annotation the server let through, or ❤ + ❤️ from the same person)
        if !g.senders.contains(&r.sender) {
            g.senders.push(r.sender.clone());
            g.who.insert(0, if r.name.is_empty() { r.sender.clone() } else { r.name.clone() });
        }
        if r.sender == me {
            if !g.mine {
                g.mine = true;
                g.rid = r.event_id.clone(); // my first one (the web's v.users.get(me))
            }
            // every one of mine in the group (❤ and ❤️ both sent): taking it back redacts them all (review)
            if !r.event_id.is_empty() {
                g.mine_ids.push(r.event_id.clone());
            }
        }
    }
    order
        .iter()
        .filter_map(|n| groups.get(n))
        .map(|g| {
            serde_json::json!({ "key": g.key, "count": g.senders.len(), "mine": g.mine, "reactionId": g.rid, "reactionIds": g.mine_ids, "who": g.who })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vectors() -> serde_json::Value {
        let path = format!("{}/src/testvectors/reactions.json", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))).unwrap()
    }

    #[test]
    fn norm_vectors() {
        let v = vectors();
        let cases = v["norm"].as_array().unwrap();
        assert!(cases.len() >= 10);
        for c in cases {
            assert_eq!(react_key_norm(c["in"].as_str().unwrap()), c["out"].as_str().unwrap(), "{}", c["in"]);
        }
    }

    #[test]
    fn group_vectors() {
        let v = vectors();
        let cases = v["groups"].as_array().unwrap();
        assert!(cases.len() >= 8);
        for c in cases {
            let name = c["name"].as_str().unwrap();
            let me = c["me"].as_str().unwrap();
            let rs: Vec<ReactIn> = c["reactions"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(i, r)| ReactIn {
                    key: r["key"].as_str().unwrap().to_string(),
                    sender: r["sender"].as_str().unwrap().to_string(),
                    event_id: format!("$r{i}"),
                    // the same display name for everyone: grouping must go by sender, never by name
                    name: "Ann".to_string(),
                })
                .collect();
            let got: Vec<serde_json::Value> = group_chips(&rs, me)
                .into_iter()
                .map(|c| serde_json::json!({ "key": c["key"], "count": c["count"], "mine": c["mine"] }))
                .collect();
            assert_eq!(serde_json::Value::Array(got), c["expect"], "{name}");
        }
    }

    #[test]
    fn mine_takes_back_my_first() {
        let rs = vec![
            ReactIn { key: "❤".into(), sender: "@me:x".into(), event_id: "$old".into(), name: "Me".into() },
            ReactIn { key: "❤️".into(), sender: "@me:x".into(), event_id: "$new".into(), name: "Me".into() },
        ];
        let c = group_chips(&rs, "@me:x");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0]["reactionId"], "$old");
        assert_eq!(c[0]["reactionIds"], serde_json::json!(["$old", "$new"]), "taking it back redacts both");
        assert_eq!(c[0]["who"], serde_json::json!(["Me"]));
    }
}
