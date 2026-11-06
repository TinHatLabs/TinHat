//! Which user a LiveKit participant identity belongs to (identity fix 12.1.8, share
//! claude/tinhat-security-review/IDENTITY-PARSE-20261102.md, finding B). MIRROR: the web's
//! src/features/calls/identity.ts (same rules; the same vectors, `testvectors/identity.json`, owned by the web and
//! copied byte for byte).
//!
//! A Tin Hat call server mints `<mxid>:<device_id>`. The mxid itself may hold one or two ':' (`@u:host`,
//! `@u:host:8448`), so splitting the string at a fixed place loses a server's port: the recorder's old
//! `record_mxid_of` kept the first two parts, so a user on `host:8448` who chose "never record me" was looked up as
//! `@u:host` and recorded anyway. The room's live call memberships say who is behind an identity: the session whose
//! `<sender>:<device_id>` is exactly the identity. Only when none matches (or two do, which only a ':' in a device id
//! can cause) does the string decide: everything before the LAST ':' (a port kept). The recorder's consent rule is
//! stricter ([`record_excluded`]: any possible owner's "no" counts).

/// `@u:server:DEVICE` → `@u:server`: everything before the last ':' when what remains still holds a ':' (the web's
/// `util.ts mxidOf`, byte for byte).
pub fn mxid_of(identity: &str) -> String {
    match identity.rfind(':') {
        Some(i) if i > 0 && identity[..i].contains(':') => identity[..i].to_string(),
        _ => identity.to_string(),
    }
}

/// 12.1.9 (assembly-044 area 3 P2): is this call participant (an identity, or a fake call's bare mxid) the user
/// `mxid`'s, by the string alone? The call menus' rule (AppCore.qml tileMxid / identityForMxid; the web's identity.ts
/// isParticipantOf): `@a:h:8448:DEV` is `@a:h:8448`'s, never `@a:h`'s.
#[allow(dead_code)] // the QML copy (AppCore.qml tileMxid) is what runs; this one carries the mirrored tests
pub fn is_participant_of(participant: &str, mxid: &str) -> bool {
    !mxid.is_empty() && (participant == mxid || mxid_of(participant) == mxid)
}

/// Users whose live call membership `(sender, device_id)` mints exactly this identity, deduped, in order.
pub fn users_of_identity(identity: &str, sessions: &[(String, String)]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (sender, device) in sessions {
        if sender.is_empty() || device.is_empty() {
            continue;
        }
        if identity.len() == sender.len() + 1 + device.len()
            && identity.starts_with(sender.as_str())
            && identity[sender.len()..].starts_with(':')
            && identity.ends_with(device.as_str())
            && !out.iter().any(|u| u == sender)
        {
            out.push(sender.clone());
        }
    }
    out
}

/// The user behind a LiveKit identity: its one live membership; otherwise [`mxid_of`].
pub fn user_of_identity(identity: &str, sessions: &[(String, String)]) -> String {
    let users = users_of_identity(identity, sessions);
    if users.len() == 1 {
        users.into_iter().next().unwrap_or_default()
    } else {
        mxid_of(identity)
    }
}

/// Is this LiveKit identity (always an identity, never a bare mxid: `@u:h:8448` reads as `@u:h`'s device `8448`) the
/// user `mxid`'s?
pub fn identity_is_user(identity: &str, mxid: &str, sessions: &[(String, String)]) -> bool {
    !mxid.is_empty() && user_of_identity(identity, sessions) == mxid
}

/// "Never record me" for an identity (the recorder: vodsvc → streamer `set_record_exclusion`; the web's
/// `recordExcluded`, same vectors): excluded when ANY user who could own it opted out, i.e. any opted-out user whose
/// `<mxid>:` starts the identity (or who is the identity). The true owner always does, whatever the device id, so a
/// port server's user is matched with their port. Deliberately not decided by the memberships: a membership claiming
/// the identity (`@a:h` with device `8448:PHONE` for `@a:h:8448:PHONE`) must not be able to switch off someone else's
/// "no"; the price is that `@a:h` saying no also keeps `@a:h:8448` (same DNS owner) out.
pub fn record_excluded(identity: &str, optouts: &[String]) -> bool {
    optouts.iter().any(|u| {
        !u.is_empty()
            && (identity == u
                || (identity.len() > u.len() && identity.starts_with(u.as_str()) && identity.as_bytes()[u.len()] == b':'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const VECTORS: &str = include_str!("testvectors/identity.json");

    fn sessions(v: &Value) -> Vec<(String, String)> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|p| (p[0].as_str().unwrap().to_string(), p[1].as_str().unwrap().to_string()))
            .collect()
    }
    fn strs(v: &Value) -> Vec<String> {
        v.as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn vectors() {
        let v: Value = serde_json::from_str(VECTORS).unwrap();
        let (user, isu, rec) = (v["user"].as_array().unwrap(), v["identityIsUser"].as_array().unwrap(), v["recordExcluded"].as_array().unwrap());
        assert!(user.len() >= 10 && isu.len() >= 7 && rec.len() >= 11);
        for c in user {
            let (id, s) = (c["identity"].as_str().unwrap(), sessions(&c["sessions"]));
            assert_eq!(users_of_identity(id, &s), strs(&c["users"]), "users: {}", c["name"]);
            assert_eq!(user_of_identity(id, &s), c["user"].as_str().unwrap(), "user: {}", c["name"]);
        }
        for c in isu {
            let got = identity_is_user(c["id"].as_str().unwrap(), c["mxid"].as_str().unwrap(), &sessions(&c["sessions"]));
            assert_eq!(got, c["want"].as_bool().unwrap(), "identityIsUser: {}", c["name"]);
        }
        for c in rec {
            let got = record_excluded(c["identity"].as_str().unwrap(), &strs(&c["optouts"]));
            assert_eq!(got, c["want"].as_bool().unwrap(), "recordExcluded: {}", c["name"]);
        }
    }

    /// The byte copy equals the web's (`TINHAT_WEB_DIR` = a tinhat-web tree, else `$HOME/tinhat-web`; skipped when
    /// neither has the file).
    #[test]
    fn vectors_are_the_webs() {
        let dir = std::env::var_os("TINHAT_WEB_DIR")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("tinhat-web")));
        let Some(dir) = dir else { return };
        let path = dir.join("src/features/calls/testvectors/identity.json");
        match std::fs::read_to_string(&path) {
            Ok(theirs) => assert!(VECTORS == theirs, "testvectors/identity.json differs from {}", path.display()),
            Err(_) => eprintln!("SKIPPED: no web vectors at {}", path.display()),
        }
    }

    /// The recorder's old rule (first two parts) on the port vector: what these vectors pin down (negative control).
    #[test]
    fn the_old_recorder_rule_missed_the_port_users_opt_out() {
        let old = |identity: &str| {
            let mut it = identity.splitn(3, ':');
            match (it.next(), it.next()) {
                (Some(a), Some(b)) => format!("{a}:{b}"),
                _ => identity.to_string(),
            }
        };
        let optouts = vec!["@carol:h.test:8448".to_string()];
        assert!(!optouts.contains(&old("@carol:h.test:8448:PHONE")), "the old rule recorded her");
        assert!(record_excluded("@carol:h.test:8448:PHONE", &optouts));
    }

    #[test]
    fn non_ascii_never_panics() {
        let s = vec![("@é:h".to_string(), "ü".to_string())];
        assert_eq!(users_of_identity("@é:h:ü", &s), vec!["@é:h".to_string()]);
        assert!(users_of_identity("@é:h:üx", &s).is_empty());
        assert!(!record_excluded("é", &["@é:h".to_string()]));
        assert!(record_excluded("@é:h:ü", &["@é:h".to_string()]));
        assert_eq!(mxid_of(":"), ":");
    }

    #[test]
    fn menu_participant_keeps_the_port() {
        // 12.1.9: mirrored in the web's identity.test.ts ("menu: …")
        assert!(is_participant_of("@a:h:8448:DEV", "@a:h:8448"));
        assert!(!is_participant_of("@a:h:8448:DEV", "@a:h"));
        assert!(is_participant_of("@a:h:PHONE", "@a:h"));
        assert!(!is_participant_of("@a:h:PHONE", "@a:h:8448"));
        assert!(is_participant_of("@a:h:DEV~watch-0123456789abcdef", "@a:h"));
        assert!(is_participant_of("@a:h", "@a:h"));
        assert!(!is_participant_of("@a:h:DEV", ""));
        assert!(!is_participant_of("@ab:h:DEV", "@a:h"));
    }
}
