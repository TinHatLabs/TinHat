//! Device clocks and verification requests (fix/verify-request, 2026-11-12), plus the plain-words notes the verify
//! dialog shows around them. Shared word for word with Tin Hat Web src/core/matrix/clockPolicy.ts (same vectors in
//! both test suites). No Qt, no SDK.
//!
//! Why: matrix-sdk-crypto (both apps) silently DROPS an incoming m.key.verification.request whose `timestamp` (the
//! SENDER's clock) is more than 10 minutes older or 5 minutes newer than the receiver's clock
//! (VerificationMachine::is_timestamp_valid). Nothing goes back to the sender, so a device with a wrong clock waits
//! forever in both directions ("request state -> Created" and nothing after). A PC that dual-boots Linux and Windows
//! is the usual way to get there (Windows reads the UTC hardware clock as local time).

/// The SDK ignores a request older than this (seconds)...
pub const SDK_TOO_OLD_S: i64 = 600;
/// ...or this far ahead (seconds).
pub const SDK_TOO_NEW_S: i64 = 300;
/// Our clock vs the server's: warn at two minutes either way (well inside both SDK limits, so we warn before it bites;
/// launch-gate 2026-11-12). The warning never blocks anything.
pub const WARN_SKEW_S: i64 = 120;
/// An outgoing request with no answer after this long gets the "what to check" note.
pub const NO_ANSWER_MS: u64 = 30_000;

/// Would matrix-sdk-crypto drop a request stamped `request_ts_ms` (sender's clock) that arrives at `now_ms`
/// (receiver's clock)? The SDK's own rule, in whole seconds.
pub fn request_dropped_for_time(request_ts_ms: i64, now_ms: i64) -> bool {
    let ts = request_ts_ms.div_euclid(1000);
    let now = now_ms.div_euclid(1000);
    now - ts > SDK_TOO_OLD_S || ts - now > SDK_TOO_NEW_S
}

/// Is this device's clock far enough from the server's (`skew_s` = ours minus the server's) to break verification?
pub fn clock_off(skew_s: i64) -> bool {
    skew_s.abs() >= WARN_SKEW_S
}

/// "about 5 hours", "about 1 hour", "about 12 minutes", "about 2 days" (rounded, never "0").
pub fn about(secs: i64) -> String {
    let s = secs.abs();
    let (n, unit) = if s >= 36 * 3600 {
        ((s + 43_200) / 86_400, "day")
    } else if s >= 90 * 60 {
        ((s + 1800) / 3600, "hour")
    } else {
        (((s + 30) / 60).max(1), "minute")
    };
    format!("about {n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// "just now", "5 minutes ago", "3 hours ago", "2 days ago" — a device's last activity.
pub fn ago(secs: i64) -> String {
    let s = secs.max(0);
    if s < 90 {
        return "just now".to_string();
    }
    let (n, unit) = if s >= 36 * 3600 {
        ((s + 43_200) / 86_400, "day")
    } else if s >= 90 * 60 {
        ((s + 1800) / 3600, "hour")
    } else {
        ((s + 30) / 60, "minute")
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

const WINDOWS_TIME_HINT: &str = " On Windows: Settings → Time & language → Date & time → Sync now.";

/// This device's own clock is off (checked against the server before a request goes out).
pub fn own_clock_note(skew_s: i64, windows: bool) -> String {
    format!(
        "This device's clock is {} off, so verifying with your other devices may not work. Set the time automatically (date, time and time zone), then try again.{}",
        about(skew_s),
        if windows { WINDOWS_TIME_HINT } else { "" }
    )
}

/// An own device asked to verify, but the SDK dropped it for its timestamp. `gap_s` = the request's stamp minus our
/// clock; `own_skew_s` = our clock minus the server's when known. Then the sender's own skew is gap + own, so the note
/// can say WHICH clock is off — or, when neither is, that the request arrived late (sent while this device was offline).
pub fn peer_clock_note(peer: &str, gap_s: i64, own_skew_s: Option<i64>, windows: bool) -> String {
    let p = peer.trim();
    let who = if p.is_empty() { "Another of your devices" } else { p };
    let hint = if windows { WINDOWS_TIME_HINT } else { "" };
    let Some(own) = own_skew_s else {
        return format!(
            "{who} asked to verify, but its clock and this device's are {} apart, so the request can't be used. Set the time automatically on both, then try again.",
            about(gap_s)
        );
    };
    // the sender's skew vs the server, IF the request came straight through (an old request may just be late)
    let theirs = gap_s + own;
    if clock_off(own) {
        // "both off" only when it is AHEAD: a late delivery could explain a sender that only looks behind
        return if clock_off(theirs) && theirs > 0 {
            format!(
                "{who} asked to verify, but both clocks are off (this device's by {}, that one's by {}), so the request can't be used. Set the time automatically on both, then try again.{hint}",
                about(own),
                about(theirs)
            )
        } else {
            format!(
                "{who} asked to verify, but this device's clock is {} off, so the request can't be used. Set the time automatically here, then try again.{hint}",
                about(own)
            )
        };
    }
    if gap_s > 0 {
        // stamped in our future: only its clock can explain that
        return format!(
            "{who} asked to verify, but its clock is {} off, so the request can't be used. Set the time automatically on that device (date, time and time zone), then try again.",
            about(theirs)
        );
    }
    // too old: sent while this device was offline, or that device's clock is behind — say both
    format!(
        "{who} asked to verify, but the request is {} old, so it can't be used: it was sent while this device was offline, or that device's clock is behind. Ask again from {who} while both devices are open, and check its clock.",
        about(gap_s)
    )
}

/// An own device asked to verify, but this device doesn't have that device's keys yet (the SDK drops it too).
pub fn peer_unknown_note(peer: &str) -> String {
    let p = peer.trim();
    let who = if p.is_empty() { "Another of your devices" } else { p };
    format!(
        "{who} asked to verify, but this device hasn't caught up with it yet. Try again in a moment, or start from here: Settings → Security → that session → Verify this session."
    )
}

/// While an outgoing request waits (replaces the old "Open Tin Hat on a device you're already signed in to…").
pub const NOTE_WAITING_OUT: &str = "Open Tin Hat on another of your signed-in devices and accept there. Or start it from Tin Hat on a phone or in a browser: Settings → Security → this session → Verify this session.";

/// `names` = the verified devices, most recently seen first, already labelled ("Phone, seen 5 minutes ago").
/// Joined "A", "A or B", "A, B or C" (at most three).
pub fn device_list(names: &[String]) -> String {
    let n: Vec<&str> = names.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).take(3).collect();
    match n.len() {
        0 => String::new(),
        1 => n[0].to_string(),
        _ => format!("{} or {}", n[..n.len() - 1].join(", "), n[n.len() - 1]),
    }
}

/// One device's label for device_list: its name (else its id) and when it was last seen (`seen_ago_s` None = unknown).
pub fn device_label(name: &str, id: &str, seen_ago_s: Option<i64>) -> String {
    let n = name.trim();
    let base = if n.is_empty() { id.trim() } else { n };
    match seen_ago_s {
        Some(s) => format!("{base} (active {})", ago(s)),
        None => base.to_string(),
    }
}

/// The outgoing request got no answer in NO_ANSWER_MS.
pub fn no_answer_note(names: &[String]) -> String {
    let list = device_list(names);
    let open = if list.is_empty() {
        "Open Tin Hat on another of your signed-in devices and accept there".to_string()
    } else {
        format!("Open Tin Hat on {list} and accept there")
    };
    format!(
        "No answer yet. {open}, or start it from Tin Hat on a phone or in a browser: Settings → Security → this session → Verify this session. If nothing shows up there, check that the date, time and time zone are right on both devices."
    )
}

/// A wrong Security Key, in words a person can use.
pub const NOTE_WRONG_KEY: &str = "That Security Key doesn't match your account. Check it for typos, or use the newest one you saved.";
/// Something that isn't a Security Key at all.
pub const NOTE_NOT_A_KEY: &str = "That doesn't look like a Security Key. Check it for typos: it's 48 letters and numbers in groups of four.";

/// The verify-with-Security-Key failure line for a raw error (the SDK's / the web's). Unknown errors keep the raw
/// text after "Verification failed:" so a report still carries it.
pub fn key_error_note(raw: &str) -> String {
    let r = raw.trim();
    let l = r.to_ascii_lowercase();
    if l.contains("mac check") || l.contains("invalid mac") || l.contains("doesn't match this account") {
        return NOTE_WRONG_KEY.to_string();
    }
    const NOT_A_KEY: [&str; 7] =
        ["doesn't look like a security key", "parity", "invalid prefix", "incorrect prefix", "key has an invalid length", "incorrect length", "base58"];
    if NOT_A_KEY.iter().any(|w| l.contains(w)) {
        return NOTE_NOT_A_KEY.to_string();
    }
    if r.is_empty() {
        return "Verification failed.".to_string();
    }
    format!("Verification failed: {r}")
}

/// HTTP `Date` header (IMF-fixdate, "Thu, 12 Nov 2026 21:05:00 GMT") → milliseconds since the epoch. None = not that.
pub fn parse_http_date(s: &str) -> Option<i64> {
    let p: Vec<&str> = s.split_whitespace().collect();
    if p.len() != 6 || p[5] != "GMT" || !p[0].ends_with(',') {
        return None;
    }
    let day: i64 = p[1].parse().ok()?;
    const M: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let month = M.iter().position(|m| *m == p[2])? as i64 + 1;
    let year: i64 = p[3].parse().ok()?;
    let digits = |x: &str| !x.is_empty() && x.len() <= 4 && x.bytes().all(|b| b.is_ascii_digit());
    if !digits(p[1]) || !digits(p[3]) {
        return None;
    }
    let t: Vec<i64> = p[4].split(':').map(|x| if digits(x) { x.parse().ok() } else { None }).collect::<Option<Vec<i64>>>()?;
    if t.len() != 3 || !(1..=31).contains(&day) || t[0] > 23 || t[1] > 59 || t[2] > 60 || !(1970..=9999).contains(&year) {
        return None;
    }
    // days from civil (Howard Hinnant)
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 86_400) + t[0] * 3600 + t[1] * 60 + t[2]) * 1000)
}

/// Our clock minus the server's, in whole seconds (rounded to the nearest, halves away from zero, so +119.5 and -119.5
/// treat alike), from a response's Date header and the local times the request was sent and answered (the header has
/// 1 s resolution; the midpoint absorbs the round trip). None = no usable header.
pub fn skew_from_date(date_header: &str, sent_ms: i64, got_ms: i64) -> Option<i64> {
    let server = parse_http_date(date_header)?;
    let mid = sent_ms + (got_ms - sent_ms) / 2;
    // the header truncates to the second: the server's true time is server..server+999
    let d = mid - (server + 500);
    Some(if d >= 0 { (d + 500) / 1000 } else { -((-d + 500) / 1000) })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_794_517_500_000; // Thu, 12 Nov 2026 21:05:00 GMT

    /// SHARED VECTORS — identical in tinhat-web src/core/matrix/clockPolicy.test.ts.
    const DROPPED: &[(&str, i64, bool)] = &[
        // (name, request stamp minus receiver now in ms, want dropped)
        ("same time", 0, false),
        ("9 min old", -540_000, false),
        ("10 min old", -600_000, false),
        ("10 min 1 s old", -601_000, true),
        ("4 min ahead", 240_000, false),
        ("5 min ahead", 300_000, false),
        ("5 min 1 s ahead", 301_000, true),
        ("sender 5 h ahead (the report)", 5 * 3_600_000, true),
        ("sender 10 h behind", -10 * 3_600_000, true),
    ];

    #[test]
    fn dropped_shared_vectors() {
        for (name, d, want) in DROPPED {
            assert_eq!(request_dropped_for_time(NOW + d, NOW), *want, "{name}");
        }
    }

    const ABOUT: &[(i64, &str)] = &[
        (0, "about 1 minute"),
        (59, "about 1 minute"),
        (240, "about 4 minutes"),
        (-750, "about 13 minutes"),
        (5399, "about 90 minutes"),
        (5400, "about 2 hours"),
        (18_000, "about 5 hours"),
        (-36_000, "about 10 hours"),
        (129_600, "about 2 days"),
    ];
    const AGO: &[(i64, &str)] = &[
        (-5, "just now"),
        (60, "just now"),
        (300, "5 minutes ago"),
        (3600, "60 minutes ago"),
        (7200, "2 hours ago"),
        (200_000, "2 days ago"),
    ];

    #[test]
    fn about_and_ago_shared_vectors() {
        for (s, want) in ABOUT {
            assert_eq!(about(*s), *want, "about {s}");
        }
        for (s, want) in AGO {
            assert_eq!(ago(*s), *want, "ago {s}");
        }
    }

    #[test]
    fn clock_off_shared_vectors() {
        for (s, want) in [(0, false), (119, false), (120, true), (-120, true), (18_000, true), (-100, false)] {
            assert_eq!(clock_off(s), want, "{s}");
        }
    }

    #[test]
    fn notes_shared_vectors() {
        assert_eq!(
            own_clock_note(18_000, true),
            "This device's clock is about 5 hours off, so verifying with your other devices may not work. Set the time automatically (date, time and time zone), then try again. On Windows: Settings → Time & language → Date & time → Sync now."
        );
        assert_eq!(
            own_clock_note(-900, false),
            "This device's clock is about 15 minutes off, so verifying with your other devices may not work. Set the time automatically (date, time and time zone), then try again."
        );
        assert_eq!(
            peer_clock_note("Windows PC", 18_000, Some(3), false),
            "Windows PC asked to verify, but its clock is about 5 hours off, so the request can't be used. Set the time automatically on that device (date, time and time zone), then try again."
        );
        assert_eq!(
            peer_clock_note(" ", -18_000, None, false),
            "Another of your devices asked to verify, but its clock and this device's are about 5 hours apart, so the request can't be used. Set the time automatically on both, then try again."
        );
        assert_eq!(
            peer_clock_note("Phone", -18_000, Some(18_005), true),
            "Phone asked to verify, but this device's clock is about 5 hours off, so the request can't be used. Set the time automatically here, then try again. On Windows: Settings → Time & language → Date & time → Sync now."
        );
        assert_eq!(
            peer_clock_note("Phone", -14_400, Some(18_000), false),
            "Phone asked to verify, but both clocks are off (this device's by about 5 hours, that one's by about 60 minutes), so the request can't be used. Set the time automatically on both, then try again."
        );
        assert_eq!(
            peer_clock_note("Phone", -660, Some(2), false),
            "Phone asked to verify, but the request is about 11 minutes old, so it can't be used: it was sent while this device was offline, or that device's clock is behind. Ask again from Phone while both devices are open, and check its clock."
        );
        // our clock 5 h fast, a right-stamped request 3 minutes late: only OUR clock is named
        assert_eq!(
            peer_clock_note("Phone", -18_180, Some(18_000), false),
            "Phone asked to verify, but this device's clock is about 5 hours off, so the request can't be used. Set the time automatically here, then try again."
        );
        assert_eq!(
            peer_unknown_note("ABCDEFGHIJ"),
            "ABCDEFGHIJ asked to verify, but this device hasn't caught up with it yet. Try again in a moment, or start from here: Settings → Security → that session → Verify this session."
        );
        let names = vec![
            device_label("Phone", "AAA", Some(300)),
            device_label("", "BBBBBBBBBB", None),
            device_label("Linux desktop", "CCC", Some(7200)),
            device_label("Old laptop", "DDD", Some(9_999_999)),
        ];
        assert_eq!(device_list(&names), "Phone (active 5 minutes ago), BBBBBBBBBB or Linux desktop (active 2 hours ago)");
        assert_eq!(device_list(&names[..1]), "Phone (active 5 minutes ago)");
        assert_eq!(device_list(&names[..2]), "Phone (active 5 minutes ago) or BBBBBBBBBB");
        assert_eq!(
            no_answer_note(&names[..2]),
            "No answer yet. Open Tin Hat on Phone (active 5 minutes ago) or BBBBBBBBBB and accept there, or start it from Tin Hat on a phone or in a browser: Settings → Security → this session → Verify this session. If nothing shows up there, check that the date, time and time zone are right on both devices."
        );
        assert_eq!(
            no_answer_note(&[]),
            "No answer yet. Open Tin Hat on another of your signed-in devices and accept there, or start it from Tin Hat on a phone or in a browser: Settings → Security → this session → Verify this session. If nothing shows up there, check that the date, time and time zone are right on both devices."
        );
    }

    /// SHARED VECTORS — the raw errors seen from the SDK (desktop) and from crypto.ts (web).
    const KEY_ERRORS: &[(&str, &str)] = &[
        ("The MAC check for the secret m.cross_signing.master failed", NOTE_WRONG_KEY),
        ("that Security Key doesn't match this account", NOTE_WRONG_KEY),
        ("that doesn't look like a Security Key", NOTE_NOT_A_KEY),
        ("Could not decrypt the secret using the secret storage key, invalid MAC.", NOTE_WRONG_KEY),
        ("The parity byte of the secret storage key doesn't match: expected 1, got 2", NOTE_NOT_A_KEY),
        ("The Base58 decoded key has an invalid length, expected 35, got 12", NOTE_NOT_A_KEY),
        ("invalid length 3, expected a sequence", "Verification failed: invalid length 3, expected a sequence"),
        ("Incorrect parity", NOTE_NOT_A_KEY),
        ("network timeout", "Verification failed: network timeout"),
        ("", "Verification failed."),
    ];

    #[test]
    fn key_error_shared_vectors() {
        for (raw, want) in KEY_ERRORS {
            assert_eq!(key_error_note(raw), *want, "{raw}");
        }
        assert_eq!(NOTE_WRONG_KEY, "That Security Key doesn't match your account. Check it for typos, or use the newest one you saved.");
    }

    #[test]
    fn http_date_shared_vectors() {
        assert_eq!(parse_http_date("Thu, 12 Nov 2026 21:05:00 GMT"), Some(NOW));
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(parse_http_date("Tue, 29 Feb 2028 12:00:00 GMT"), Some(1_835_438_400_000));
        assert_eq!(parse_http_date("Thu, 12 Nov 2026 21:05:00 PST"), None);
        assert_eq!(parse_http_date("garbage"), None);
        assert_eq!(parse_http_date(""), None);
        assert_eq!(parse_http_date("Thu, 12 Nov 2026 21:-1:00 GMT"), None);
        assert_eq!(parse_http_date("Thu, 12 Nov 99999 21:05:00 GMT"), None);
        assert_eq!(parse_http_date("Thu, +2 Nov 2026 21:05:00 GMT"), None);
        // a PC 5 h fast that asked at NOW+5h and got the answer 400 ms later
        assert_eq!(skew_from_date("Thu, 12 Nov 2026 21:05:00 GMT", NOW + 18_000_000, NOW + 18_000_400), Some(18_000));
        // ±119.5 s round alike (both warn)
        assert_eq!(skew_from_date("Thu, 12 Nov 2026 21:05:00 GMT", NOW + 120_000, NOW + 120_000), Some(120));
        assert_eq!(skew_from_date("Thu, 12 Nov 2026 21:05:00 GMT", NOW - 119_000, NOW - 119_000), Some(-120));
        assert_eq!(skew_from_date("Thu, 12 Nov 2026 21:05:00 GMT", NOW + 300, NOW + 700), Some(0));
        assert_eq!(skew_from_date("nope", 0, 0), None);
    }
}
