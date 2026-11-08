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

/// 12.1.11 (assembly-047): do two call ids (each an identity or a bare mxid, in either order) name the same
/// participant: the same string, or one is the other's user ([`is_participant_of`]). The call UI's display matches
/// (levels, away, "can they hear me?", speaking: AppCore.qml sameParticipant; the web's identity.ts sameParticipant).
/// Two devices of one person are two participants. Replaces the `"x:"` prefix matches that let `@a:h` claim
/// `@a:h:8448:DEV`.
#[allow(dead_code)] // the QML copy (AppCore.qml sameParticipant) is what runs; this one carries the mirrored tests
pub fn same_participant(a: &str, b: &str) -> bool {
    !a.is_empty() && !b.is_empty() && (a == b || is_participant_of(a, b) || is_participant_of(b, a))
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
        // 12.1.11 (assembly-047): the call UI's display matches (AppCore.qml runs the same groups: qml-identity-test.qml)
        let (po, same) = (v["participantOf"].as_array().unwrap(), v["sameParticipant"].as_array().unwrap());
        assert!(po.len() >= 15 && same.len() >= 14);
        for c in po {
            let got = is_participant_of(c["participant"].as_str().unwrap(), c["mxid"].as_str().unwrap());
            assert_eq!(got, c["want"].as_bool().unwrap(), "participantOf: {}", c["name"]);
        }
        for c in same {
            let got = same_participant(c["a"].as_str().unwrap(), c["b"].as_str().unwrap());
            assert_eq!(got, c["want"].as_bool().unwrap(), "sameParticipant: {}", c["name"]);
        }
    }

    /// The call UI's old display rule (`a === b || a.indexOf(b + ":") === 0 || b.indexOf(a + ":") === 0`) fails the
    /// sameParticipant vectors on the port user (negative control: what the vectors pin down).
    #[test]
    fn the_old_prefix_rule_fails_the_port_vectors() {
        let old = |a: &str, b: &str| a == b || a.starts_with(&format!("{b}:")) || b.starts_with(&format!("{a}:"));
        let v: Value = serde_json::from_str(VECTORS).unwrap();
        let wrong: Vec<&str> = v["sameParticipant"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| old(c["a"].as_str().unwrap(), c["b"].as_str().unwrap()) != c["want"].as_bool().unwrap())
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        assert!(wrong.len() >= 3, "the old rule should miss the port vectors: {wrong:?}");
        assert!(!same_participant("@a:h:8448:PC", "@a:h"));
    }

    /// No QML file matches call identities by a `"x:"` prefix (`s.indexOf(x + ":") === 0`, `startsWith(x + ":")`):
    /// that is how `@a:h` claimed `@a:h:8448:DEV` (levels, away, "can they hear me?", speaking, the recorder; fixed
    /// 12.1.11). Use AppCore.qml isParticipantOf / sameParticipant (= this file's rules) instead.
    #[test]
    fn qml_has_no_prefix_identity_match() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("qml");
        let hits = qml_prefix_matches(&dir);
        assert!(hits.is_empty(), "\"x:\" prefix identity matches in QML (use AppCore isParticipantOf/sameParticipant):\n{}", hits.join("\n"));
    }

    fn qml_prefix_matches(dir: &std::path::Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "qml") {
                    let text = std::fs::read_to_string(&p).unwrap_or_default();
                    for (i, line) in text.lines().enumerate() {
                        let code = line.split("//").next().unwrap_or("");
                        let squashed: String = code.chars().filter(|c| !c.is_whitespace()).collect();
                        let prefix = squashed.contains("+\":\")===0")
                            || squashed.contains("+':')===0")
                            || (squashed.contains("startsWith(") && (squashed.contains("+\":\")") || squashed.contains("+':')")));
                        if prefix {
                            out.push(format!("{}:{}: {}", p.display(), i + 1, line.trim()));
                        }
                    }
                }
            }
        }
        out.sort();
        out
    }

    /// The scanner itself finds the old forms (so an empty result above means something).
    #[test]
    fn qml_prefix_scanner_sees_the_old_forms() {
        let dir = std::env::temp_dir().join(format!("qml-prefix-scan-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("A.qml"), "x\n  if (a === p || p.indexOf(a + \":\") === 0) return\n").unwrap();
        std::fs::write(dir.join("sub/B.qml"), "return id.startsWith(u + ':')\nvar lp = s.split(\":\")[0]\n// p.indexOf(u + \":\") === 0 in a comment\n").unwrap();
        let hits = qml_prefix_matches(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(hits.len(), 2, "{hits:?}");
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
