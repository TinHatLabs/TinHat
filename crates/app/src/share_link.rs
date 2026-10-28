//! share_link.rs — the links Tin Hat hands out: "Copy link" on a room or space, Share, a message's "Copy link"
//! (12.0.6, "Tin Hat links, not matrix.to"). SHARED LOGIC with Tin Hat Web's `src/core/links.ts` (the rules, line for
//! line); both replay `testvectors/share-links.json` (the web keeps a byte copy in `testvectors/share-links/vectors.json`).
//!
//! A room on the signed-in server opens in that server's own web app: `<web base>/#/room/<alias or id>[/<event id>]`,
//! the parts encoded like `encodeURIComponent` (Tin Hat Web's router.ts decodes exactly that). The web base is the
//! server's well-known `dev.tinhat.server.web` (an https origin), else the origin of its announced `api`; never a
//! built-in name. A room on another server (federation), or a server that announces neither, keeps a matrix.to link:
//! we don't know that server's web app.
//!
//! `WRAP_OPEN_LINKS` (off until tinhat.dev serves `/open` and its app-link files): an own-server link whose host is
//! not under tinhat.me goes out as `https://tinhat.dev/open#<link>`, so the phone apps (which claim tinhat.dev/open
//! and *.tinhat.me) open it. tinhat.me hosts are claimed directly and are never wrapped; matrix.to links never are.
//!
//! The read side (a pasted link of any of these shapes, the wrapped one included) is room.rs `parse_join_link`;
//! its cases are in the same vectors file (section `parse`).

/// Wrap own-server links in `https://tinhat.dev/open#…`. OFF until tinhat.dev serves `/open` + its app-link files
/// and the apps that unwrap the form are out (server docs/DEEPLINKS.md "Rollout order").
pub const WRAP_OPEN_LINKS: bool = false;

/// The claimed link that carries a server's own link in its fragment (the phone apps and tinhat.dev/open read it).
pub const OPEN_PREFIX: &str = "https://tinhat.dev/open#";

/// `encodeURIComponent`, byte for byte (UTF-8, upper-case hex).
pub fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        let keep = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')');
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `decodeURIComponent`, except that where it would throw (a `%` without two hex digits after it, bytes that aren't
/// UTF-8) the text comes back as it is. Tin Hat Web's `decodeOrKeep`.
pub fn decode_component(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = |c: u8| (c as char).to_digit(16);
            match (b.get(i + 1).copied().and_then(hex), b.get(i + 2).copied().and_then(hex)) {
                (Some(h), Some(l)) => {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                }
                _ => return s.to_string(),
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// JS `\s` exactly (what `String.prototype.trim` strips): the shared whitespace of these rules, so both apps trim and
/// refuse the same characters (Rust's own `char::is_whitespace` differs: it has U+0085 and lacks U+FEFF).
pub fn js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}'
            | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

/// `s.trim()` in JS.
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(js_space)
}

/// A host[:port] as a link may carry it: a DNS-style host (letters, digits, `.` and `-`) or a bracketed IPv6 literal,
/// then an optional port 1–65535. No user info, path, spaces or anything else. Server names are checked the same way
/// before they become a `via` hint.
pub fn authority_ok(a: &str) -> bool {
    let (host_ok, rest) = match a.strip_prefix('[').and_then(|r| r.find(']')) {
        Some(end) => (ipv6_ok(&a[1..1 + end]), &a[end + 2..]),
        None => {
            let end = a.find(':').unwrap_or(a.len());
            let host = &a[..end];
            (!host.is_empty() && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'), &a[end..])
        }
    };
    if !host_ok {
        return false;
    }
    match rest.strip_prefix(':') {
        None => rest.is_empty(),
        Some(p) => (1..=5).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u32>().is_ok_and(|n| (1..=65535).contains(&n)),
    }
}

/// An IPv6 literal's inside (no brackets): eight groups of 1–4 hex digits, or fewer around one `::` (no dotted IPv4
/// tail). A linear scan (the web's ipv6Ok, no regex).
fn ipv6_ok(inner: &str) -> bool {
    if inner.is_empty() || !inner.bytes().all(|b| b.is_ascii_hexdigit() || b == b':') {
        return false;
    }
    // every group 1–4 digits; "" = no groups (either side of a `::` may be empty)
    let groups_ok = |s: &str| s.is_empty() || s.split(':').all(|g| (1..=4).contains(&g.len()));
    let count = |s: &str| if s.is_empty() { 0 } else { s.split(':').count() };
    match inner.split_once("::") {
        Some((l, r)) => !r.contains("::") && groups_ok(l) && groups_ok(r) && count(l) + count(r) <= 7,
        None => count(inner) == 8 && groups_ok(inner),
    }
}

/// `s` without a case-insensitive `prefix`.
fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s.is_char_boundary(prefix.len()) && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

/// Where this server's web app lives (no trailing slash), or "" when it doesn't say:
/// 1. `web` (`dev.tinhat.server.web`): an https origin only (`https://host[:port]`, one trailing `/` allowed; no
///    path, query, fragment or user info);
/// 2. else the origin of `api` (`dev.tinhat.server.api`, http or https: a home-network server may be plain http).
pub fn web_base(web: &str, api: &str) -> String {
    let w = js_trim(web);
    if let Some(rest) = strip_prefix_ci(w, "https://") {
        let rest = rest.strip_suffix('/').unwrap_or(rest);
        if authority_ok(rest) {
            return format!("https://{}", rest.to_ascii_lowercase());
        }
    }
    let a = js_trim(api);
    for scheme in ["https", "http"] {
        if let Some(rest) = strip_prefix_ci(a, &format!("{scheme}://")) {
            let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
            if authority_ok(authority) {
                return format!("{scheme}://{}", authority.to_ascii_lowercase());
            }
            return String::new();
        }
    }
    String::new()
}

/// The server part of a room id / alias / user id: everything after the first `:` ("" for a room id without one,
/// room version 12).
pub fn server_part(id: &str) -> &str {
    id.split_once(':').map(|(_, s)| s).unwrap_or("")
}

/// Is the link's host claimed by the phone apps directly (`tinhat.me` and its names)? `base` = `scheme://host[:port]`.
fn tinhat_me_host(base: &str) -> bool {
    let authority = base.split_once("://").map(|(_, a)| a).unwrap_or(base);
    let host = if authority.starts_with('[') { authority } else { authority.split(':').next().unwrap_or("") };
    host == "tinhat.me" || host.ends_with(".tinhat.me")
}

/// What a link is made from (the vectors' fields).
#[derive(Clone, Debug, Default)]
pub struct LinkInput<'a> {
    /// the room id (`!opaque:server`, or `!opaque` from room version 12)
    pub room: &'a str,
    /// its published alias, "" for none (a room link prefers it; a message link always names the room id)
    pub alias: &'a str,
    /// the event id for a message link, "" for a room link
    pub event: &'a str,
    /// the signed-in account's server name
    pub own: &'a str,
    /// the well-known's `dev.tinhat.server.web`, "" when absent
    pub web: &'a str,
    /// the well-known's `dev.tinhat.server.api`, "" when absent
    pub api: &'a str,
    /// WRAP_OPEN_LINKS (a parameter so the vectors cover both)
    pub wrap: bool,
}

/// The link to share for a room, a space or a message ("" when there is no room id).
pub fn share_link(i: &LinkInput) -> String {
    let base = web_base(i.web, i.api);
    share_link_on(i, &base)
}

/// `share_link` with the web base already chosen (Tin Hat Web passes its own page's origin).
pub fn share_link_on(i: &LinkInput, base: &str) -> String {
    let room = js_trim(i.room);
    let alias = js_trim(i.alias);
    let event = js_trim(i.event);
    let me = js_trim(i.own);
    let target = if event.is_empty() && !alias.is_empty() { alias } else { room };
    if target.is_empty() {
        return String::new();
    }
    let server = server_part(target);
    // someone signed in, and the room is on their server (ASCII case only; a room-version-12 id has no server part: we
    // are in the room, so our server can serve it)
    let own = !me.is_empty() && (server.is_empty() || server.eq_ignore_ascii_case(me));
    let mut path = encode_component(target);
    if !event.is_empty() {
        path.push('/');
        path.push_str(&encode_component(event));
    }
    if own && !base.is_empty() {
        let link = format!("{base}/#/room/{path}");
        if i.wrap && base.starts_with("https://") && !tinhat_me_host(base) {
            return format!("{OPEN_PREFIX}{link}");
        }
        return link;
    }
    // matrix.to: a room id gets a `via` hint (a bare id leaves the other client no server to ask), only a server name
    // that can be one (never `&`, `=`, … into the query)
    let via = if target.starts_with('!') {
        if server.is_empty() { me } else { server }
    } else {
        ""
    };
    let via = if authority_ok(via) { via } else { "" };
    if via.is_empty() {
        format!("https://matrix.to/#/{path}")
    } else {
        format!("https://matrix.to/#/{path}?via={via}")
    }
}

/// The signed-in session's link for a room / space / message (the offer's web base; matrix.to for other servers).
pub fn share_link_now(room: &str, alias: &str, event: &str) -> String {
    let own = crate::session::server_name().unwrap_or_default();
    let o = crate::server_offer::current();
    share_link(&LinkInput { room, alias, event, own: &own, web: &o.web, api: &o.api, wrap: WRAP_OPEN_LINKS })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::room::{parse_join_link, JoinTarget};
    use serde_json::Value;

    const VECTORS: &str = include_str!("testvectors/share-links.json");

    fn s<'a>(c: &'a Value, k: &str) -> &'a str {
        c[k].as_str().unwrap_or("")
    }

    #[test]
    fn share_link_shared_vectors() {
        let v: Value = serde_json::from_str(VECTORS).unwrap();
        let cases = v["links"].as_array().unwrap();
        assert!(cases.len() >= 20);
        for c in cases {
            let i = LinkInput {
                room: s(c, "room"),
                alias: s(c, "alias"),
                event: s(c, "event"),
                own: s(c, "own"),
                web: s(c, "web"),
                api: s(c, "api"),
                wrap: c["wrap"].as_bool().unwrap_or(false),
            };
            assert_eq!(share_link(&i), s(c, "want"), "{c}");
        }
    }

    #[test]
    fn web_base_shared_vectors() {
        let v: Value = serde_json::from_str(VECTORS).unwrap();
        for c in v["web_base"].as_array().unwrap() {
            assert_eq!(web_base(s(c, "web"), s(c, "api")), s(c, "want"), "{c}");
        }
    }

    /// The read side: every link the apps hand out (and matrix.to's, and the wrapped form) joins the right room.
    #[test]
    fn parse_shared_vectors() {
        let v: Value = serde_json::from_str(VECTORS).unwrap();
        for c in v["parse"].as_array().unwrap() {
            let (kind, got) = match parse_join_link(s(c, "input")) {
                Ok(JoinTarget::RoomOrAlias { id, via }) => ("room", Some((id, via))),
                Ok(JoinTarget::Person(_)) => ("person", None),
                Err(_) => ("none", None),
            };
            let want = c["target"].as_str().map(|t| {
                let via = c["via"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
                (t.to_string(), via)
            });
            assert_eq!(kind, s(c, "kind"), "{c}");
            assert_eq!(got, want, "{c}");
        }
    }

    /// What we hand out reads back as the same room (every own-server and matrix.to case of the vectors).
    #[test]
    fn links_round_trip_through_parse() {
        let v: Value = serde_json::from_str(VECTORS).unwrap();
        for c in v["links"].as_array().unwrap() {
            let want = s(c, "want");
            if want.is_empty() {
                continue;
            }
            let alias = js_trim(s(c, "alias"));
            let target = if js_trim(s(c, "event")).is_empty() && !alias.is_empty() { alias } else { js_trim(s(c, "room")) };
            match parse_join_link(want) {
                Ok(JoinTarget::RoomOrAlias { id, .. }) => assert_eq!(id, target, "{c}"),
                other => panic!("{want} → {other:?}"),
            }
        }
    }

    #[test]
    fn decode_component_is_decode_uri_component_or_the_text() {
        assert_eq!(decode_component("%23caf%C3%A9%3Ax"), "#café:x");
        assert_eq!(decode_component("a%ZZb"), "a%ZZb");
        assert_eq!(decode_component("a%2"), "a%2");
        assert_eq!(decode_component("%C3"), "%C3");
        assert_eq!(decode_component("plain"), "plain");
        assert_eq!(decode_component("%e2%82%ac"), "€");
    }

    #[test]
    fn wrap_is_off_until_tinhat_dev_serves_open() {
        assert!(!WRAP_OPEN_LINKS);
    }

    /// The byte copy in Tin Hat Web equals ours when a web checkout is named by `TINHAT_WEB_DIR` (skipped when unset;
    /// a named checkout without the file fails).
    #[test]
    fn vectors_are_the_webs() {
        let Some(dir) = std::env::var_os("TINHAT_WEB_DIR") else { return };
        let path = std::path::Path::new(&dir).join("testvectors/share-links/vectors.json");
        let theirs = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(VECTORS, theirs);
    }

    #[test]
    fn authority_and_space_rules() {
        for ok in ["example.com", "example.com:443", "[fd00::1]", "[fd00::1]:8448", "[::]", "[1:2:3:4:5:6:7:8]", "localhost", "10.0.0.1:65535"] {
            assert!(authority_ok(ok), "{ok}");
        }
        for bad in ["", "[broken", "example.com:abc", "example.com:65536", "example.com:0", "example.com:", "fd00::1", "a b", "u@h", "ex_ample.com", "[fd00::1]x", "[]", "[:]", "[:::]", "[1:2]", "[1::2::3]"] {
            assert!(!authority_ok(bad), "{bad}");
        }
        assert_eq!(js_trim("\u{feff} x \u{3000}"), "x");
        assert!(!js_space('\u{85}'));
    }
}
