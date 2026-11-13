//! Reply and Mark as read on a message notification (docs/NOTIF-ACTIONS.md) — the pure rules. The web runs the same
//! rules (`src/features/push/notifactions.ts`) against the same vectors (`testvectors/notifactions.json`,
//! byte-identical in both repos).
//!
//!   offer        what a message notification carries ("default" = the click, then "reply" / "inline-reply", "read")
//!   outcome      an invoked key → what the app does
//!   reply_content / read_markers / failed_body   what is sent / shown
//!
//! The transports (freedesktop notifications, WinRT toasts) live in `notify.rs`; the sends in [`run`].

use serde_json::{json, Value};

/// Trimmed from both ends of a reply (after CRLF / lone CR became LF): JavaScript's `trim()` set plus U+0085, so the
/// desktop and the web cut exactly the same characters.
pub(crate) const WHITESPACE: &[char] = &[
    '\u{09}', '\u{0A}', '\u{0B}', '\u{0C}', '\u{0D}', '\u{20}', '\u{85}', '\u{A0}', '\u{1680}', '\u{2000}', '\u{2001}',
    '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}', '\u{2006}', '\u{2007}', '\u{2008}', '\u{2009}', '\u{200A}',
    '\u{2028}', '\u{2029}', '\u{202F}', '\u{205F}', '\u{3000}', '\u{FEFF}',
];

pub(crate) const WORD_REPLY: &str = "Reply";
pub(crate) const WORD_MARK_READ: &str = "Mark as read";
pub(crate) const WORD_SEND: &str = "Send";
pub(crate) const WORD_PLACEHOLDER: &str = "Message";
pub(crate) const WORD_FAILED_TITLE: &str = "Your reply wasn't sent";
const FAILED_TEXT_MAX: usize = 120;

/// The typed text as it is sent: CRLF and lone CR → LF, then trimmed (see [`WHITESPACE`]).
pub(crate) fn normalise(text: &str) -> String {
    let t = text.replace("\r\n", "\n").replace('\r', "\n");
    t.trim_matches(|c| WHITESPACE.contains(&c)).to_string()
}

/// The `m.room.message` content a reply sends: a plain body, no formatting, no relation, empty intentional mentions
/// (a quick reply never pings anyone). None = nothing to send.
pub(crate) fn reply_content(text: &str) -> Option<Value> {
    let body = normalise(text);
    if body.is_empty() {
        return None;
    }
    Some(json!({ "msgtype": "m.text", "body": body, "m.mentions": {} }))
}

/// An event id a notification may act on: '$' + 1..254 more UTF-8 bytes, no control or white-space characters.
pub(crate) fn event_ok(e: &str) -> bool {
    e.starts_with('$')
        && (2..=255).contains(&e.len())
        && !e.chars().any(|c| c.is_control() || c.is_whitespace() || WHITESPACE.contains(&c))
}

/// The `POST /rooms/{room}/read_markers` body for Mark as read: both markers on the notified event. None = no Mark
/// as read (there is no fallback to the room's newest event: that could acknowledge an unseen message).
/// `public_ok` = `receipts::public_receipt_ok` for the room (THE RULE in receipts.rs: a public `m.read` only in a room
/// of my own server, `foreign_room::is_managed`; anywhere else, a missing create event included, `m.read.private`).
/// Fail-private on purpose: rooms on connected Tin Hat servers get the private receipt too until open-network slice 1
/// narrows it with its connected list.
pub(crate) fn read_markers(event: &str, public_ok: bool) -> Option<Value> {
    event_ok(event).then(|| crate::receipts::markers_body(event, public_ok))
}

/// What the notification server / toast can do and what this device may do.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OfferCtx<'a> {
    /// buttons at all (freedesktop `actions` capability; always on Windows)
    pub actions_cap: bool,
    /// an inline text field (freedesktop `inline-reply`; Windows 10 1903+)
    pub inline_field: bool,
    /// this device is cross-signed by the account's own identity
    pub trusted: bool,
    /// the notified event ("" = unknown)
    pub event: &'a str,
}

/// The action keys a message notification carries, in order.
pub(crate) fn offer(c: &OfferCtx) -> Vec<&'static str> {
    if !c.actions_cap {
        return vec!["default"];
    }
    let mut out = vec!["default", if c.inline_field && c.trusted { "inline-reply" } else { "reply" }];
    if event_ok(c.event) {
        out.push("read");
    }
    out
}

/// What an invoked action does.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Outcome {
    /// open the room (the click)
    Open,
    /// open the room with the composer focused
    Compose,
    /// send `content`, then the markers when present
    Send { content: Value, then_read: Option<Value> },
    /// send the markers
    Read { markers: Value },
    /// post the failure note with this body
    Failed { note: String },
    None,
}

/// An invoked `key` (with the typed `text` for "inline-reply"), `trusted_now` = the device's trust checked at this
/// moment AND the screen not locked (fail closed: unknown trust = false), `public_ok` = may the room get a public
/// receipt (see [`read_markers`]; unknown = false).
pub(crate) fn outcome(key: &str, text: &str, event: &str, trusted_now: bool, public_ok: bool) -> Outcome {
    match key {
        "default" => Outcome::Open,
        "reply" => Outcome::Compose,
        "inline-reply" => match reply_content(text) {
            None => Outcome::Compose,
            Some(_) if !trusted_now => Outcome::Failed { note: failed_body(text) },
            Some(content) => Outcome::Send { content, then_read: read_markers(event, public_ok) },
        },
        "read" => match read_markers(event, public_ok) {
            Some(markers) => Outcome::Read { markers },
            None => Outcome::None,
        },
        _ => Outcome::None,
    }
}

/// The failure note's body (the iOS words): the first 120 characters of what was typed.
pub(crate) fn failed_body(text: &str) -> String {
    let t: String = normalise(text).chars().take(FAILED_TEXT_MAX).collect();
    format!("Open Tin Hat to send it: \u{201C}{t}\u{201D}")
}

/// The pending entry a posted message notification leaves behind (`notify.rs` keys it by the notification id):
/// "msg:<session generation>\t<room>\t<event>".
pub(crate) fn pending_entry(gen: u64, room: &str, event: &str) -> String {
    format!("msg:{gen}\t{room}\t{event}")
}

pub(crate) fn parse_pending(s: &str) -> Option<(u64, String, String)> {
    let rest = s.strip_prefix("msg:")?;
    let mut it = rest.splitn(3, '\t');
    let gen = it.next()?.parse().ok()?;
    let room = it.next()?.to_string();
    let event = it.next()?.to_string();
    room.starts_with('!').then_some((gen, room, event))
}

// ─────────────── Windows: the toast's XML (pure, tested everywhere) ───────────────

/// XML text / attribute escaping for the toast template (every interpolated string goes through it).
#[cfg_attr(not(windows), allow(dead_code))] // Windows toasts (the tests run it everywhere)
pub(crate) fn xml_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            // XML 1.0 Char = #x9 | #xA | #xD | [#x20-#xD7FF] | [#xE000-#xFFFD] | [#x10000-#x10FFFF]: anything else (other C0
            // controls, U+FFFE / U+FFFF) would make LoadXml refuse the whole toast — a space instead
            c if !matches!(c as u32, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF) => o.push(' '),
            c => o.push(c),
        }
    }
    o
}

/// A message toast with the [`offer`]ed keys: the click = launch "default"; "inline-reply" = a text box + Send;
/// "reply" = a Reply button (opens the room); "read" = Mark as read. All handled in-process (Activated).
#[cfg_attr(not(windows), allow(dead_code))] // Windows toasts (the tests run it everywhere)
pub(crate) fn toast_xml(title: &str, body: &str, icon_path: Option<&str>, keys: &[&str]) -> String {
    let mut x = String::from(r#"<toast launch="default" activationType="foreground"><visual><binding template="ToastGeneric">"#);
    x.push_str(&format!("<text>{}</text><text>{}</text>", xml_escape(title), xml_escape(body)));
    if let Some(p) = icon_path.filter(|p| !p.is_empty()) {
        x.push_str(&format!(r#"<image placement="appLogoOverride" hint-crop="circle" src="{}"/>"#, xml_escape(p)));
    }
    x.push_str("</binding></visual>");
    let buttons: Vec<&str> = keys.iter().copied().filter(|k| *k != "default").collect();
    if !buttons.is_empty() {
        x.push_str("<actions>");
        if buttons.contains(&"inline-reply") {
            x.push_str(&format!(r#"<input id="reply" type="text" placeHolderContent="{}"/>"#, xml_escape(WORD_PLACEHOLDER)));
        }
        for k in buttons {
            let (label, extra) = match k {
                "inline-reply" => (WORD_SEND, r#" hint-inputId="reply""#),
                "read" => (WORD_MARK_READ, ""),
                _ => (WORD_REPLY, ""),
            };
            x.push_str(&format!(r#"<action content="{}" arguments="{}" activationType="foreground"{}/>"#, xml_escape(label), k, extra));
        }
        x.push_str("</actions>");
    }
    x.push_str("</toast>");
    x
}

// ─────────────── the sends (the app's own paths) ───────────────

/// Is this device cross-signed by the account's own identity right now? Asked fresh; any error or unknown = false.
pub(crate) async fn own_device_trusted(client: &matrix_sdk::Client) -> bool {
    match client.encryption().get_own_device().await {
        Ok(Some(d)) => d.is_verified_with_cross_signing(),
        _ => false,
    }
}

/// Is the session's screen locked right now? (PM rule 2026-11-07: no reply from a locked session.) Linux: the
/// freedesktop / GNOME ScreenSaver `GetActive` (KDE, GNOME, most lockers), each call bounded at 1 s; no screen saver
/// service answering = not locked (there is no lock screen to reply from). Windows: the input desktop can't be
/// opened while the lock screen (or another secure desktop) has it = locked.
#[cfg(target_os = "linux")]
pub(crate) async fn session_locked() -> bool {
    // the reply itself arrived over the session bus: no bus now = something is wrong = fail closed
    let Some(conn) = crate::notify::connection().await else { return true };
    let Ok(dbus) = zbus::fdo::DBusProxy::new(&conn).await else { return true };
    let mut any_locker = false;
    for (dest, paths, iface) in [
        ("org.freedesktop.ScreenSaver", &["/ScreenSaver", "/org/freedesktop/ScreenSaver"][..], "org.freedesktop.ScreenSaver"),
        ("org.gnome.ScreenSaver", &["/org/gnome/ScreenSaver"][..], "org.gnome.ScreenSaver"),
    ] {
        let Ok(name) = zbus::names::BusName::try_from(dest) else { return true };
        match tokio::time::timeout(std::time::Duration::from_secs(1), dbus.name_has_owner(name)).await {
            Ok(Ok(false)) => continue,     // no such locker on this desktop
            Ok(Ok(true)) => any_locker = true,
            _ => return true,              // can't tell: fail closed
        }
        let mut answered = false;
        for path in paths {
            let call = conn.call_method(Some(dest), *path, Some(iface), "GetActive", &());
            if let Ok(Ok(m)) = tokio::time::timeout(std::time::Duration::from_secs(1), call).await {
                if let Ok(active) = m.body().deserialize::<bool>() {
                    if active {
                        return true;
                    }
                    answered = true;
                    break;
                }
            }
        }
        if !answered {
            return true; // a locker that doesn't answer: fail closed
        }
    }
    if !any_locker {
        eprintln!("[notify] no screen saver service on the session bus: no lock screen to reply from");
    }
    false
}

#[cfg(windows)]
pub(crate) async fn session_locked() -> bool {
    #[link(name = "user32")]
    extern "system" {
        fn OpenInputDesktop(flags: u32, inherit: i32, access: u32) -> isize;
        fn CloseDesktop(desktop: isize) -> i32;
    }
    const DESKTOP_SWITCHDESKTOP: u32 = 0x0100;
    // SAFETY: plain Win32 calls; a desktop handle we opened is closed again.
    let h = unsafe { OpenInputDesktop(0, 0, DESKTOP_SWITCHDESKTOP) };
    if h == 0 {
        return true;
    }
    unsafe { CloseDesktop(h) };
    false
}

#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) async fn session_locked() -> bool {
    false
}

/// The joined room of the CURRENT session for an action of session `gen` (None: signed out, another session, gone).
fn room_for(gen: u64, room_id: &str) -> Option<(matrix_sdk::Client, matrix_sdk::Room)> {
    if crate::session::generation() != gen {
        eprintln!("[notify] action of an ended session ignored");
        return None;
    }
    let client = crate::session::client()?;
    let rid = matrix_sdk::ruma::RoomId::parse(room_id).ok()?;
    let room = client.get_room(&rid)?;
    (room.state() == matrix_sdk::RoomState::Joined).then_some((client, room))
}

/// The content [`reply_content`] describes, as the SDK type the composer's send takes (tested equal to the JSON).
pub(crate) fn reply_event_content(body: &str) -> matrix_sdk::ruma::events::room::message::RoomMessageEventContent {
    use matrix_sdk::ruma::events::{room::message::RoomMessageEventContent, Mentions};
    RoomMessageEventContent::text_plain(body).add_mentions(Mentions::new())
}

/// Run an invoked action of a message notification. `open` routes to QML ("open:<room>" / "compose:<room>").
pub(crate) async fn run(gen: u64, room_id: String, event: String, key: String, text: String) {
    // the trust is asked only for a send (and fresh); a locked screen counts as not trusted (no reply from a lock
    // screen: the text goes into the failure note, nothing is sent)
    let trusted_now = if key == "inline-reply" && reply_content(&text).is_some() {
        match room_for(gen, &room_id) {
            Some((client, _)) => {
                let trusted = own_device_trusted(&client).await;
                let locked = session_locked().await;
                if locked {
                    eprintln!("[notify] reply not sent: the session is locked");
                }
                trusted && !locked
            }
            None => false,
        }
    } else {
        false
    };
    // the receipt rule (receipts.rs): asked of the room; no room / no create event = private. The send itself goes
    // through receipts::send_read_markers, which decides the same way again at the moment it sends.
    let public_ok = if key == "read" || key == "inline-reply" {
        match room_for(gen, &room_id) {
            Some((_, room)) => crate::receipts::public_receipt_here(&room).await,
            None => false,
        }
    } else {
        false
    };
    let out = outcome(&key, &text, &event, trusted_now, public_ok);
    if matches!(out, Outcome::Open | Outcome::Compose) && crate::session::generation() != gen {
        eprintln!("[notify] action of an ended session ignored");
        return;
    }
    match out {
        Outcome::Open => crate::notify::toast_action(format!("open:{room_id}")),
        Outcome::Compose => crate::notify::toast_action(format!("compose:{room_id}")),
        Outcome::None => {}
        Outcome::Read { .. } => {
            let Some((_, room)) = room_for(gen, &room_id) else { return };
            match crate::receipts::send_read_markers(&room, &event).await {
                Ok(()) => eprintln!("[notify] mark as read: markers sent"),
                Err(e) => eprintln!("[notify] mark as read failed: {e}"),
            }
        }
        Outcome::Failed { note } => {
            eprintln!("[notify] reply not sent: not trusted, locked, or the session changed");
            failed(gen, &room_id, &note).await;
        }
        Outcome::Send { content, then_read } => {
            let Some((_, room)) = room_for(gen, &room_id) else {
                failed(gen, &room_id, &failed_body(&text)).await;
                return;
            };
            let body = content.get("body").and_then(|b| b.as_str()).unwrap_or_default().to_string();
            match room.send(reply_event_content(&body)).await {
                Ok(r) => {
                    eprintln!("[notify] reply sent: {}", r.response.event_id);
                    if then_read.is_some() && crate::session::generation() == gen {
                        if let Err(e) = crate::receipts::send_read_markers(&room, &event).await {
                            eprintln!("[notify] reply sent, mark as read failed: {e}");
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[notify] reply failed: {e}");
                    failed(gen, &room_id, &failed_body(&text)).await;
                }
            }
        }
    }
}

async fn failed(gen: u64, room_id: &str, note: &str) {
    if crate::session::generation() != gen {
        return; // never a note about another account's room
    }
    crate::notify::notify_action(WORD_FAILED_TITLE, note, None, &format!("open:{room_id}")).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vectors() -> Value {
        serde_json::from_str(include_str!("testvectors/notifactions.json")).unwrap()
    }

    #[test]
    fn whitespace_and_words_match_the_vectors() {
        let v = vectors();
        let ws: Vec<u32> = v["whitespace"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u32).collect();
        let mine: Vec<u32> = WHITESPACE.iter().map(|c| *c as u32).collect();
        assert_eq!(ws, mine);
        let w = &v["words"];
        assert_eq!(w["reply"], WORD_REPLY);
        assert_eq!(w["markRead"], WORD_MARK_READ);
        assert_eq!(w["send"], WORD_SEND);
        assert_eq!(w["placeholder"], WORD_PLACEHOLDER);
        assert_eq!(w["failedTitle"], WORD_FAILED_TITLE);
        assert_eq!(w["failedTextMax"], FAILED_TEXT_MAX as u64);
    }

    #[test]
    fn reply_content_vectors() {
        let v = vectors();
        for c in v["replyContent"].as_array().unwrap() {
            let got = reply_content(c["text"].as_str().unwrap()).unwrap_or(Value::Null);
            assert_eq!(got, c["want"], "{c}");
        }
    }

    #[test]
    fn the_sdk_content_is_the_vector_content() {
        let v = vectors();
        for c in v["replyContent"].as_array().unwrap() {
            if c["want"].is_null() {
                continue;
            }
            let body = c["want"]["body"].as_str().unwrap();
            let got = serde_json::to_value(reply_event_content(body)).unwrap();
            assert_eq!(got, c["want"], "{c}");
        }
    }

    #[test]
    fn read_markers_vectors() {
        let v = vectors();
        for c in v["readMarkers"].as_array().unwrap() {
            // these cases are a room of my own server
            let got = c["event"].as_str().and_then(|e| read_markers(e, true)).unwrap_or(Value::Null);
            assert_eq!(got, c["want"], "{c}");
        }
    }

    /// The receipt rule (receipts.rs): a public m.read only in a room of my own server; another server, an unknown
    /// create event, a v12 room with a creator elsewhere → m.read.private + m.fully_read.
    #[test]
    fn receipt_rule_vectors() {
        let v = vectors();
        let cases = v["receiptRule"].as_array().unwrap();
        assert_eq!(cases.len(), 14);
        let (mut public, mut private) = (0, 0);
        for c in cases {
            let create = Some(&c["create"]).filter(|x| !x.is_null());
            let ok = crate::receipts::public_receipt_ok(create, c["own"].as_str().unwrap());
            let got = c["event"].as_str().and_then(|e| read_markers(e, ok)).unwrap_or(Value::Null);
            assert_eq!(got, c["want"], "receiptRule: {}", c["name"]);
            if got.get("m.read").is_some() {
                public += 1;
            }
            if got.get("m.read.private").is_some() {
                private += 1;
            }
        }
        // the loop isn't vacuous either way
        assert!(public >= 3 && private >= 9, "{public} public, {private} private");
    }

    /// The vectors are the web's byte for byte (TINHAT_WEB_DIR = the web checkout; unset → ~/tinhat-web, skipped loudly
    /// when missing).
    #[test]
    fn vectors_are_the_webs() {
        let mine = include_str!("testvectors/notifactions.json");
        let named = std::env::var_os("TINHAT_WEB_DIR").map(std::path::PathBuf::from);
        let dir = named.clone().or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("tinhat-web")));
        let Some(dir) = dir else {
            eprintln!("SKIPPED: no TINHAT_WEB_DIR and no HOME: the web vectors weren't compared");
            return;
        };
        let path = dir.join("src/features/push/testvectors/notifactions.json");
        match std::fs::read_to_string(&path) {
            Ok(theirs) => assert!(mine == theirs, "testvectors/notifactions.json differs from {}", path.display()),
            // a checkout named on purpose must hold the file
            Err(e) if named.is_some() => panic!("TINHAT_WEB_DIR is set but {} is unreadable: {e}", path.display()),
            Err(_) => eprintln!("SKIPPED: no web vectors at {}", path.display()),
        }
    }

    #[test]
    fn offer_vectors() {
        let v = vectors();
        for c in v["offer"].as_array().unwrap() {
            let x = &c["ctx"];
            let ctx = OfferCtx {
                actions_cap: x["actionsCap"].as_bool().unwrap(),
                inline_field: x["inlineField"].as_bool().unwrap(),
                trusted: x["trusted"].as_bool().unwrap(),
                event: x["event"].as_str().unwrap(),
            };
            let want: Vec<&str> = c["want"].as_array().unwrap().iter().map(|k| k.as_str().unwrap()).collect();
            assert_eq!(offer(&ctx), want, "{c}");
        }
    }

    fn outcome_json(o: &Outcome) -> Value {
        match o {
            Outcome::Open => json!({ "do": "open" }),
            Outcome::Compose => json!({ "do": "compose" }),
            Outcome::None => json!({ "do": "none" }),
            Outcome::Read { markers } => json!({ "do": "read", "markers": markers }),
            Outcome::Failed { note } => json!({ "do": "failed", "note": note }),
            Outcome::Send { content, then_read } => {
                let mut j = json!({ "do": "send", "content": content });
                if let Some(m) = then_read {
                    j["thenRead"] = m.clone();
                }
                j
            }
        }
    }

    #[test]
    fn outcome_vectors() {
        let v = vectors();
        for c in v["outcome"].as_array().unwrap() {
            let x = &c["case"];
            let got = outcome(
                x["key"].as_str().unwrap(),
                x.get("text").and_then(|t| t.as_str()).unwrap_or(""),
                x["event"].as_str().unwrap(),
                x["trustedNow"].as_bool().unwrap(),
                x["publicReceipt"].as_bool().unwrap(),
            );
            assert_eq!(outcome_json(&got), c["want"], "{c}");
        }
    }

    #[test]
    fn failed_note_vectors() {
        let v = vectors();
        for c in v["failedNote"].as_array().unwrap() {
            assert_eq!(failed_body(c["text"].as_str().unwrap()), c["want"].as_str().unwrap(), "{c}");
        }
    }

    #[test]
    fn the_windows_toast_escapes_everything_and_carries_the_offered_keys() {
        let x = toast_xml("<Ana & \"Bo\">", "hi </text><action/> \u{1}x", Some("C:\\a&b.png"), &["default", "inline-reply", "read"]);
        assert!(x.contains("<text>&lt;Ana &amp; &quot;Bo&quot;&gt;</text>"), "{x}");
        assert!(x.contains("<text>hi &lt;/text&gt;&lt;action/&gt;  x</text>"), "{x}");
        assert!(x.contains(r#"src="C:\a&amp;b.png""#), "{x}");
        assert!(x.contains(r#"<input id="reply" type="text" placeHolderContent="Message"/>"#), "{x}");
        assert!(x.contains(r#"<action content="Send" arguments="inline-reply" activationType="foreground" hint-inputId="reply"/>"#), "{x}");
        assert!(x.contains(r#"<action content="Mark as read" arguments="read" activationType="foreground"/>"#), "{x}");
        assert_eq!(x.matches("<action ").count(), 2);
        let y = toast_xml("t", "b", None, &["default", "reply"]);
        assert!(!y.contains("<input"), "{y}");
        assert!(y.contains(r#"<action content="Reply" arguments="reply" activationType="foreground"/>"#), "{y}");
        assert!(!y.contains("<image"), "{y}");
        let w = toast_xml("a\u{FFFE}b\u{FFFF}c\u{7}d", "ok\r\n", None, &["default"]);
        assert!(w.contains("<text>a b c d</text><text>ok\r\n</text>"), "{w}");
        let z = toast_xml("t", "b", None, &["default"]);
        assert!(!z.contains("<actions>"), "{z}");
    }

    #[test]
    fn pending_entries_round_trip_and_refuse_junk() {
        let p = pending_entry(7, "!r:example.com", "$e");
        assert_eq!(parse_pending(&p), Some((7, "!r:example.com".into(), "$e".into())));
        assert_eq!(parse_pending("open:!r:example.com"), None);
        assert_eq!(parse_pending("msg:x\t!r\t$e"), None);
        assert_eq!(parse_pending("msg:1\tnotaroom\t$e"), None);
        assert_eq!(parse_pending(&pending_entry(1, "!r:example.com", "")), Some((1, "!r:example.com".into(), String::new())));
    }
}
