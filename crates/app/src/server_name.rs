//! server_name — what a typed Server field means (sign-in, create account, password reset, the moved check). A bare
//! name — ASCII letters, digits and inner hyphens; no dot, no colon, no scheme — is a tinhat.me name: "crew" →
//! crew.tinhat.me, as the phone apps' resolver does. Anything else is left as typed (trimmed): a domain of its own, a
//! host with a port, a full https:// (or, on the desktop, http://) address.
//! Shared table: servername.cases.json, byte-identical with Tin Hat Web's src/core/servername.cases.json (sha256 pinned
//! in both tests); its `origin` column is the phone apps' rule, for their lanes to adopt.

/// The names zone a bare name belongs to.
pub const NAME_ZONE: &str = "tinhat.me";

/// A bare name: 1+ ASCII letters / digits / hyphens, not starting or ending with a hyphen.
fn is_bare_name(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') && !s.starts_with('-') && !s.ends_with('-')
}

/// The typed text with a bare name made a tinhat.me name ("crew" → "crew.tinhat.me", "crew/#/x" →
/// "crew.tinhat.me/#/x"); anything else comes back trimmed and otherwise unchanged.
pub fn expand(typed: &str) -> String {
    let s = typed.trim();
    if s.contains("://") {
        return s.to_string();
    }
    let end = s.find(['/', '#', '?']).unwrap_or(s.len());
    let (host, rest) = s.split_at(end);
    if is_bare_name(host) {
        format!("{}.{NAME_ZONE}{rest}", host.to_ascii_lowercase())
    } else {
        s.to_string()
    }
}

/// The host the sign-in screen asks for its moved and password-reset checks: the expanded text without its http(s)://
/// and trailing slashes, lower case. None = nothing usable typed (empty, a path or link, a space, another scheme, a
/// user name, a character no host name has). `[…]` = an IPv6 address. Trimming is Unicode White_Space (`str::trim`),
/// lower case ASCII-only: the web's servername.ts does exactly the same (shared table).
pub fn host(typed: &str) -> Option<String> {
    let e = expand(typed).to_ascii_lowercase();
    let h = e.strip_prefix("https://").or_else(|| e.strip_prefix("http://")).unwrap_or(&e).trim_end_matches('/');
    let ok = !h.is_empty()
        && h.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'))
        && !h.starts_with(['-', '.', ':'])
        && !h.ends_with('-');
    ok.then(|| h.to_string())
}

/// The quiet line under the Server field while a bare name is typed ("Opens crew.tinhat.me", the phone apps' words);
/// empty for anything else.
pub fn hint(typed: &str) -> String {
    let t = typed.trim();
    let e = expand(t);
    if e == t {
        return String::new();
    }
    match host(t) {
        Some(h) => format!("Opens {h}"),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CASES: &str = include_str!("servername.cases.json");
    /// sha256 of servername.cases.json — the same pin as Tin Hat Web's servername.test.ts.
    const CASES_SHA256: &str = "a2954e3ed36a67a9f045d87fd4d96423620fdae35535996f9f78e1587a642b4e";

    #[test]
    fn the_case_table_is_the_shared_one() {
        use sha2::Digest;
        let got: String = sha2::Sha256::digest(CASES.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(got, CASES_SHA256, "servername.cases.json must be byte-identical with Tin Hat Web's");
    }

    #[test]
    fn every_shared_case() {
        let v: serde_json::Value = serde_json::from_str(CASES).unwrap();
        assert_eq!(v["zone"], NAME_ZONE);
        let cases = v["cases"].as_array().unwrap();
        assert!(cases.len() >= 20);
        for c in cases {
            let name = c["name"].as_str().unwrap();
            let input = c["input"].as_str().unwrap();
            assert_eq!(expand(input), c["expanded"].as_str().unwrap(), "expand: {name}");
            assert_eq!(host(input).as_deref(), c["host"].as_str(), "host: {name}");
        }
    }

    #[test]
    fn the_hint_names_the_expanded_server_only_for_a_bare_name() {
        assert_eq!(hint("crew"), "Opens crew.tinhat.me");
        assert_eq!(hint(" Crew-2 "), "Opens crew-2.tinhat.me");
        assert_eq!(hint("crew.tinhat.me"), "");
        assert_eq!(hint("chat.example.com"), "");
        assert_eq!(hint("crew/#/invite/abc"), "", "a link: nothing to name as a host");
        assert_eq!(hint(""), "");
        assert_eq!(hint("-crew"), "");
    }
}
