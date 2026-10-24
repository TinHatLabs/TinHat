//! Quiet push rules (10.3). Matrix's per-room "All messages" is a `room`-kind push rule, and a
//! room rule matches EVERY event in the room — call membership state (each join or leave is an
//! `org.matrix.msc3401.call.member` state event), the roles reconciler's power-level writes,
//! Element Call's key-distribution events, our own calendar / watch-party / soundboard state.
//! Synapse counted each of those as an unread notification, so a room set to All messages chimed
//! and grew a badge during every call with nobody typing (a field report: dozens of "missed messages"
//! with the chat read to the bottom — most of them were call joins and leaves).
//!
//! The fix lives on the ACCOUNT so every client of it benefits, web included: user-defined
//! override rules with no actions, one per housekeeping event type. Override rules run before
//! room rules, and user-defined overrides run before the server defaults, so
//! `.m.rule.suppress_edits` and `.m.rule.reaction` keep doing their job after ours. Ensured once
//! per login after the first sync (the ruleset is only known then); rules already present are
//! left alone, so a normal start costs nothing on the wire.

pub const RULE_PREFIX: &str = "dev.tinhat.quiet.";

/// (rule id suffix, `type` glob). Message-bearing types (`m.room.message`, `m.room.encrypted`,
/// `m.sticker`) and membership (`m.room.member` — the server's own `.m.rule.member_event` and
/// `.m.rule.invite_for_me` own it) are deliberately absent. `org.matrix.msc4075.*` (Element
/// Call's "ring" events) stay too: they are the one housekeeping event that means "someone
/// wants you".
pub const QUIET_TYPES: &[(&str, &str)] = &[
    ("call_member", "org.matrix.msc3401.call.member"),
    ("call_keys", "io.element.call.encryption_keys"),
    ("power_levels", "m.room.power_levels"),
    ("app_state", "dev.tinhat.*"), // the app's own state: calendar, roles, watch party, record, guests, …
    ("space", "m.space.*"),
    ("pinned", "m.room.pinned_events"),
    ("name", "m.room.name"),
    ("topic", "m.room.topic"),
    ("avatar", "m.room.avatar"),
    ("join_rules", "m.room.join_rules"),
    ("history_visibility", "m.room.history_visibility"),
    ("guest_access", "m.room.guest_access"),
    ("encryption", "m.room.encryption"),
    ("canonical_alias", "m.room.canonical_alias"),
    ("redaction", "m.room.redaction"),
];

pub fn rule_id(suffix: &str) -> String {
    format!("{RULE_PREFIX}{suffix}")
}

/// Make sure every quiet rule exists on this account. Idempotent; logs one line.
pub async fn ensure(client: &matrix_sdk::Client) {
    use matrix_sdk::ruma::push::{EventMatchConditionData, PushCondition, RuleKind};
    let ns = client.notification_settings().await;
    let have: std::collections::HashSet<String> =
        ns.ruleset().await.override_.iter().map(|r| r.rule_id.clone()).collect();
    let mut added = 0usize;
    let mut failed = 0usize;
    for (suffix, pattern) in QUIET_TYPES {
        let id = rule_id(suffix);
        if have.contains(&id) {
            continue;
        }
        let cond = PushCondition::EventMatch(EventMatchConditionData::new("type".to_owned(), (*pattern).to_owned()));
        match ns.create_custom_conditional_push_rule(id.clone(), RuleKind::Override, Vec::new(), vec![cond]).await {
            Ok(()) => added += 1,
            Err(e) => {
                failed += 1;
                eprintln!("[quietrules] {id}: {e}");
            }
        }
    }
    if added > 0 || failed > 0 {
        eprintln!("[quietrules] {added} quiet push rules added, {failed} failed ({} were already there)", QUIET_TYPES.len() - added - failed);
    } else {
        eprintln!("[quietrules] all {} quiet push rules present", QUIET_TYPES.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The subset of Matrix push-rule glob semantics we rely on: `*` = any run of characters,
    /// everything else literal, whole-value match.
    fn glob_hits(pattern: &str, value: &str) -> bool {
        fn go(p: &[u8], v: &[u8]) -> bool {
            match p.first() {
                None => v.is_empty(),
                Some(b'*') => (0..=v.len()).any(|i| go(&p[1..], &v[i..])),
                Some(c) => v.first() == Some(c) && go(&p[1..], &v[1..]),
            }
        }
        go(pattern.as_bytes(), value.as_bytes())
    }

    fn quieted(ty: &str) -> bool {
        QUIET_TYPES.iter().any(|(_, p)| glob_hits(p, ty))
    }

    #[test]
    fn ids_are_unique_and_prefixed() {
        let ids: Vec<String> = QUIET_TYPES.iter().map(|(s, _)| rule_id(s)).collect();
        let set: std::collections::HashSet<&String> = ids.iter().collect();
        assert_eq!(set.len(), ids.len(), "duplicate quiet rule id");
        for id in &ids {
            assert!(id.starts_with(RULE_PREFIX));
            assert!(!id[RULE_PREFIX.len()..].is_empty());
        }
    }

    #[test]
    fn never_quiets_messages_or_membership() {
        for ty in ["m.room.message", "m.room.encrypted", "m.sticker", "m.room.member", "m.reaction",
                   "org.matrix.msc4075.rtc.notification", "org.matrix.msc4075.call.notify", "m.room.tombstone"] {
            assert!(!quieted(ty), "{ty} must keep notifying");
        }
    }

    #[test]
    fn quiets_the_offenders_seen_on_the_server() {
        // Every event type a busy room sees that is not a message: each of these counted as unread under an
        // All-messages room rule.
        for ty in ["org.matrix.msc3401.call.member", "io.element.call.encryption_keys", "m.room.power_levels",
                   "dev.tinhat.calendar.event", "dev.tinhat.calendar.rsvp", "dev.tinhat.calendar.game",
                   "dev.tinhat.role", "dev.tinhat.member_roles", "dev.tinhat.soundboard", "dev.tinhat.games.prefs",
                   "dev.tinhat.record", "dev.tinhat.watchparty.viewer", "dev.tinhat.watchparty.playback",
                   "dev.tinhat.watchparty", "dev.tinhat.activity", "dev.tinhat.clip", "dev.tinhat.afk",
                   "dev.tinhat.guests", "dev.tinhat.guests.waiting", "m.room.redaction", "m.room.join_rules", "m.room.history_visibility",
                   "m.room.guest_access", "m.room.name", "m.room.encryption", "m.space.child", "m.space.parent",
                   "m.room.pinned_events", "m.room.topic"] {
            assert!(quieted(ty), "{ty} should be quiet");
        }
    }

    #[test]
    fn glob_helper_matches_the_spec_we_rely_on() {
        assert!(glob_hits("dev.tinhat.*", "dev.tinhat.role"));
        assert!(glob_hits("dev.tinhat.*", "dev.tinhat.afk"));
        assert!(!glob_hits("dev.tinhat.*", "dev.tinhatx.role"));
        assert!(!glob_hits("dev.tinhat.*", "com.example.thing"));
        assert!(glob_hits("m.space.*", "m.space.child"));
        assert!(!glob_hits("m.space.*", "m.spaces"));
        assert!(glob_hits("m.room.name", "m.room.name"));
        assert!(!glob_hits("m.room.name", "m.room.name2"));
    }
}
