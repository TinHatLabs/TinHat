//! limits.rs — port of `limits.ts` (ENGINE.md §4 limits, B10, §9 C2): serialized size, string length, nesting depth,
//! "no URLs", the one mxc:// exception, id syntax.

/// 8 KB = 8192 bytes of UTF-8 of the compact serialization.
pub const MAX_BYTES: usize = 8192;
/// Keys of `roles` + keys of `tokens`.
pub const MAX_OVERRIDES: usize = 400;
pub const MAX_ID: usize = 64;
/// Unicode code points, every string (keys too) at any depth.
pub const MAX_STRING: usize = 80;
/// Objects/arrays nested deeper than this are refused (root = 1).
pub const MAX_DEPTH: usize = 16;

/// Unicode code points (the reference counts a surrogate pair once).
pub fn code_points(s: &str) -> usize {
    s.chars().count()
}

/// First `max` code points of s.
pub fn take_code_points(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// Schemes that make a string a URL when they start a word (also "www."). "://" anywhere is always a URL.
pub const URL_SCHEMES: [&str; 10] =
    ["http:", "https:", "ftp:", "file:", "data:", "javascript:", "vbscript:", "mailto:", "blob:", "www."];

/// "No URLs anywhere in a theme" (§4, B10), as a portable rule: after ASCII-lowercasing, the string contains "://",
/// or one of [`URL_SCHEMES`] at the start or right after a character that is not an ASCII letter or digit.
pub fn contains_url(s: &str) -> bool {
    let t = s.to_ascii_lowercase();
    let b = t.as_bytes();
    if t.contains("://") {
        return true;
    }
    for w in URL_SCHEMES {
        let mut from = 0;
        while let Some(p) = t[from..].find(w) {
            let i = from + p;
            // a non-ASCII previous character ends in a continuation byte: not alphanumeric, like the UTF-16 unit
            if i == 0 || !b[i - 1].is_ascii_alphanumeric() {
                return true;
            }
            from = i + 1;
        }
    }
    false
}

/// JSON pointer of the one field allowed to hold a URL (ENGINE.md §9 C2).
pub const BACKDROP_IMAGE_PATH: &str = "/backdrop/image";

fn is_server_char(ch: u8) -> bool {
    ch.is_ascii_alphanumeric() || ch == b'.' || ch == b':' || ch == b'-'
}
fn is_media_char(ch: u8) -> bool {
    ch.is_ascii_alphanumeric() || ch == b'_' || ch == b'-'
}

/// `backdrop.image` (§9 C2): exactly `^mxc://[A-Za-z0-9.:-]{1,255}/[A-Za-z0-9_-]{1,255}$` — "mxc://" (lower case),
/// 1–255 server characters, one "/", 1–255 media-id characters, nothing else (no trailing newline).
/// (Byte and UTF-16 lengths differ only for non-ASCII text, which never matches.)
pub fn is_mxc_uri(s: &str) -> bool {
    let Some(rest) = s.as_bytes().strip_prefix(b"mxc://") else { return false };
    let Some(slash) = rest.iter().position(|&c| c == b'/') else { return false };
    if !(1..=255).contains(&slash) {
        return false;
    }
    let media = rest.len() - slash - 1;
    if !(1..=255).contains(&media) {
        return false;
    }
    rest[..slash].iter().all(|&c| is_server_char(c)) && rest[slash + 1..].iter().all(|&c| is_media_char(c))
}

/// Theme id: 1–64 characters of a-z 0-9 . : -
pub fn is_theme_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_ID
        && s.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'.' || c == b':' || c == b'-')
}

/// Role/token id (and expression ref): a letter, then letters, digits, . : - ; at most 64 characters (A1).
pub fn is_key_id(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= MAX_ID
        && b[0].is_ascii_alphabetic()
        && b.iter().all(|&c| c.is_ascii_alphanumeric() || c == b'.' || c == b':' || c == b'-')
}

/// JSON-pointer segment (RFC 6901 escaping), cut to 40 code points so error details stay short.
pub fn path_seg(s: &str) -> String {
    take_code_points(s, 40).replace('~', "~0").replace('/', "~1")
}
