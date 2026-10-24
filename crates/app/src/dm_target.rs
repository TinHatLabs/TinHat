//! dm_target.rs — the New DM field's rule (2026-09-29: people can type a plain username instead of a full
//! @user:example.com). Shared logic: Tin Hat Web's `src/features/nav/dmTarget.ts` applies the same rule, and both
//! replay the contract vectors `testvectors/dm-target/vectors.json` (reference: contract `tools/dm-target-vectors.py`;
//! byte copy in `src/testvectors/dm-target.json`).
//!
//!   dm_target("blake", "example.com")          → Some("@blake:example.com")
//!   dm_target("@Someone:Matrix.ORG", "…")      → Some("@someone:matrix.org")
//!   dm_target("https://matrix.to/#/@x:y.com")  → Some("@x:y.com")
//!   dm_target("two words", "…")                → None (not a user)

/// The Matrix user ID a New DM field means, or None. `own_server` = the signed-in account's server name.
pub fn dm_target(input: &str, own_server: &str) -> Option<String> {
    let mut s = input.trim();
    let lower = s.to_ascii_lowercase();
    for pre in ["https://matrix.to/#/", "http://matrix.to/#/"] {
        if lower.starts_with(pre) {
            let rest = &s[pre.len()..];
            s = rest[..rest.find('?').unwrap_or(rest.len())].trim();
            break;
        }
    }
    if s.is_empty() || s.chars().any(char::is_whitespace) {
        return None;
    }
    let s = s.strip_prefix('@').unwrap_or(s).to_lowercase();
    let (local, server) = match s.split_once(':') {
        Some((l, r)) => (l.to_string(), r.to_string()),
        None => (s.clone(), own_server.trim().to_lowercase()),
    };
    let local_ok = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || "._=-/+".contains(c);
    if local.is_empty() || !local.chars().all(local_ok) {
        return None;
    }
    let (host, port) = match server.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (server.as_str(), None),
    };
    let host_ok = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-';
    if host.is_empty() || !host.chars().all(host_ok) {
        return None;
    }
    if let Some(p) = port {
        if p.is_empty() || p.len() > 5 || !p.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
    }
    if !host.contains('.') && host != "localhost" {
        return None;
    }
    Some(format!("@{local}:{server}"))
}

/// The server part of a user ID ("@me:example.com" → "example.com"; "" when there is none).
pub fn server_of(user_id: &str) -> &str {
    user_id.split_once(':').map(|(_, s)| s).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dm_target_shared_vectors() {
        let v: serde_json::Value = serde_json::from_str(include_str!("testvectors/dm-target.json")).unwrap();
        let cases = v["cases"].as_array().unwrap();
        assert!(cases.len() >= 20);
        for c in cases {
            let want = c["want"].as_str().map(str::to_string);
            let got = dm_target(c["input"].as_str().unwrap(), c["server"].as_str().unwrap());
            assert_eq!(got, want, "{c}");
        }
    }

    /// The byte copy equals the shared contract's vectors when a contract checkout is named by
    /// `TINHAT_CONTRACT_DIR` (skipped when it is unset or the file is absent).
    #[test]
    fn vectors_are_the_contracts() {
        let Some(dir) = std::env::var_os("TINHAT_CONTRACT_DIR") else { return };
        let path = std::path::Path::new(&dir).join("testvectors/dm-target/vectors.json");
        if let Ok(theirs) = std::fs::read_to_string(path) {
            assert_eq!(include_str!("testvectors/dm-target.json"), theirs);
        }
    }

    #[test]
    fn server_of_works() {
        assert_eq!(server_of("@me:example.com"), "example.com");
        assert_eq!(server_of("@me:host:8448"), "host:8448");
        assert_eq!(server_of(""), "");
    }
}
