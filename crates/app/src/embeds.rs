//! Inline click-to-play media embeds — a faithful Rust port of the earlier
//! web client's media embeds (§H). PURE parser, no Matrix/network: scans an
//! `m.room.message` body for URLs and maps each to an embed descriptor
//! `{id, platform, mode, src, poster?, label, layout}` that the QML cards render.
//! Nothing loads from the third party until the user clicks (mode video/iframe).
//!
//! `extract_embeds(body)` and `body_is_only_embed(body)` are called by room.rs while
//! serializing the timeline, so each message JSON carries its embeds. The QML cards
//! (WebEngineView / MediaPlayer) are a later, visual-only addition; ALL branching
//! logic lives here and is covered by the unit tests at the bottom.

use serde_json::{json, Value};
use std::sync::OnceLock;
use url::Url;

const MAX_EMBEDS: usize = 3;
/// Twitch `parent=` must match the embedding origin; native QtWebEngine isn't served
/// from a web origin, and Twitch accepts `localhost` for native/dev embeds.
const TWITCH_PARENT: &str = "localhost";

pub(crate) fn url_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r#"(?i)\bhttps?://[^\s<>"')]+"#).unwrap())
}

/// Strip a rich-reply fallback: leading `> ` quote lines + one blank line after.
/// 10.1 (TH-MSG-05): `<span data-mx-spoiler[="reason"]>x</span>` — Qt's rich text knows no such
/// attribute and showed the spoiler in the clear. Render it hidden (text the colour of its own
/// background) behind a "spoiler" tag; selecting the text reveals it.
/// themes (desktop C2, contract SCAN-MISSES): the colours are the ACTIVE theme's `bubble.spoiler.tag.fg` /
/// `bubble.spoiler.bg` — this HTML is made here, so the inline style carries them as text (Qt's rich text reads
/// `#rrggbb` and `#aarrggbb` alike). A message rendered before a theme switch keeps its colours until it is re-rendered.
pub fn hide_spoilers(html: &str) -> String {
    if !html.contains("data-mx-spoiler") {
        return html.to_string();
    }
    static SPOILER_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = SPOILER_RE.get_or_init(|| {
        regex::Regex::new(r#"(?is)<span\s+data-mx-spoiler(?:="([^"]*)")?\s*>(.*?)</span>"#).unwrap()
    });
    let tag_fg = crate::theme_rt::qt_colour("bubble.spoiler.tag.fg");
    let bg = crate::theme_rt::qt_colour("bubble.spoiler.bg");
    re.replace_all(html, |c: &regex::Captures| {
        let reason = c.get(1).map(|m| m.as_str()).unwrap_or("");
        let tag = if reason.is_empty() { "spoiler".to_string() } else { format!("spoiler: {reason}") };
        format!(
            "<span style=\"color:{tag_fg}\">[{tag} — select to reveal]</span> <span style=\"background-color:{bg};color:{bg}\">{}</span>",
            &c[2]
        )
    })
    .into_owned()
}

pub fn strip_reply_fallback(text: &str) -> String {
    let mut lines = text.lines().peekable();
    let mut skipped = false;
    while let Some(l) = lines.peek() {
        if l.starts_with("> ") {
            lines.next();
            skipped = true;
        } else {
            break;
        }
    }
    if skipped {
        if let Some(l) = lines.peek() {
            if l.trim().is_empty() {
                lines.next();
            }
        }
    }
    lines.collect::<Vec<_>>().join("\n")
}

/// Extract all embeds from a message body (deduped by id, capped at MAX_EMBEDS).
pub fn extract_embeds(body: &str) -> Vec<Value> {
    let stripped = strip_reply_fallback(body);
    let mut out: Vec<Value> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for m in url_re().find_iter(&stripped) {
        let raw = m.as_str().trim_end_matches(['.', ',', '!', '?', ')', ']']);
        if let Some(embed) = parse_embed(raw) {
            let id = embed["id"].as_str().unwrap_or("").to_string();
            if id.is_empty() || seen.contains(&id) {
                continue;
            }
            seen.push(id);
            out.push(embed);
            if out.len() >= MAX_EMBEDS {
                break;
            }
        }
    }
    out
}

/// True iff the (reply-stripped) body is EXACTLY one URL whose embed is image/video
/// (a lone GIF/video link) — so the raw URL text can be hidden, showing just the card.
/// iframe-platform links (YouTube/Twitch/…) keep their URL text (return false).
pub fn body_is_only_embed(body: &str) -> bool {
    let t = strip_reply_fallback(body);
    let t = t.trim();
    if t.is_empty() || t.split_whitespace().count() != 1 {
        return false;
    }
    match parse_embed(t) {
        Some(e) => matches!(e["mode"].as_str(), Some("image") | Some("video")),
        None => false,
    }
}

fn host_of(u: &Url) -> String {
    u.host_str().unwrap_or("").trim_start_matches("www.").to_lowercase()
}

fn first_segs(u: &Url) -> Vec<String> {
    u.path_segments()
        .map(|s| s.filter(|p| !p.is_empty()).map(|p| p.to_string()).collect())
        .unwrap_or_default()
}

fn query<'a>(u: &'a Url, key: &str) -> Option<String> {
    u.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned())
}

const YT_ID_OK: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-";
fn is_yt_id(s: &str) -> bool {
    s.len() >= 6 && s.bytes().all(|b| YT_ID_OK.contains(&b))
}

/// Parse `t`/`start` (seconds, or `1h2m3s`) into total seconds.
fn parse_yt_start(u: &Url) -> Option<u64> {
    let raw = query(u, "t").or_else(|| query(u, "start"))?;
    if let Ok(n) = raw.parse::<u64>() {
        return Some(n);
    }
    let (mut total, mut cur) = (0u64, 0u64);
    let mut any = false;
    for ch in raw.chars() {
        if ch.is_ascii_digit() {
            cur = cur * 10 + (ch as u64 - '0' as u64);
        } else {
            let mult = match ch {
                'h' => 3600,
                'm' => 60,
                's' => 1,
                _ => return None,
            };
            total += cur * mult;
            cur = 0;
            any = true;
        }
    }
    if any { Some(total) } else { None }
}

fn embed(id: &str, platform: &str, mode: &str, src: &str, poster: Option<&str>, label: &str, layout: &str) -> Value {
    let mut v = json!({ "id": id, "platform": platform, "mode": mode, "src": src, "label": label, "layout": layout });
    if let Some(p) = poster {
        v["poster"] = Value::from(p);
    }
    v
}

/// Minimal percent-encode for a URL passed as a query value (SoundCloud `url=`).
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Map a single URL to an embed descriptor, or None if not embeddable.
pub fn parse_embed(raw: &str) -> Option<Value> {
    let u = Url::parse(raw).ok()?;
    if u.scheme() != "http" && u.scheme() != "https" {
        return None;
    }
    let host = host_of(&u);
    let segs = first_segs(&u);
    let yt_src = |id: &str, label: &str| -> Value {
        let mut src = format!("https://www.youtube-nocookie.com/embed/{id}?autoplay=1&rel=0");
        if let Some(s) = parse_yt_start(&u) {
            src.push_str(&format!("&start={s}"));
        }
        let poster = format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg");
        embed(&src, "youtube", "iframe", &src, Some(&poster), label, "video")
    };

    // YouTube
    if host == "youtu.be" {
        if let Some(id) = segs.first() {
            if is_yt_id(id) {
                return Some(yt_src(id, "YouTube"));
            }
        }
    }
    // Media Party 11.4: a YouTube Music link is the same video (the card says where it came from)
    if host == "youtube.com" || host == "m.youtube.com" || host == "youtube-nocookie.com" || host == "music.youtube.com" {
        if let Some(v) = query(&u, "v").filter(|v| is_yt_id(v)) {
            return Some(yt_src(&v, if host == "music.youtube.com" { "YouTube Music" } else { "YouTube" }));
        }
        if let [kind, id, ..] = segs.as_slice() {
            if (kind == "shorts" || kind == "embed" || kind == "live") && is_yt_id(id) {
                return Some(yt_src(id, "YouTube"));
            }
        }
    }
    // Vimeo
    if host == "vimeo.com" {
        if let Some(id) = segs.first().filter(|s| s.chars().all(|c| c.is_ascii_digit())) {
            let src = format!("https://player.vimeo.com/video/{id}?autoplay=1");
            return Some(embed(&src, "vimeo", "iframe", &src, None, "Vimeo", "video"));
        }
    }
    // Streamable
    if host == "streamable.com" {
        if let Some(id) = segs.first().filter(|s| *s != "e" && s.chars().all(|c| c.is_ascii_alphanumeric())) {
            let src = format!("https://streamable.com/e/{id}?autoplay=1");
            return Some(embed(&src, "streamable", "iframe", &src, None, "Streamable", "video"));
        }
    }
    // Twitch
    if host == "clips.twitch.tv" {
        if let Some(slug) = segs.first() {
            let src = format!("https://clips.twitch.tv/embed?clip={slug}&parent={TWITCH_PARENT}&autoplay=true");
            return Some(embed(&src, "twitch", "iframe", &src, None, "Twitch", "video"));
        }
    }
    if host == "twitch.tv" {
        if let [chan, "clip", slug, ..] = segs.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            let _ = chan;
            let src = format!("https://clips.twitch.tv/embed?clip={slug}&parent={TWITCH_PARENT}&autoplay=true");
            return Some(embed(&src, "twitch", "iframe", &src, None, "Twitch", "video"));
        }
        if let ["videos", id, ..] = segs.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            let src = format!("https://player.twitch.tv/?video={id}&parent={TWITCH_PARENT}&autoplay=true");
            return Some(embed(&src, "twitch", "iframe", &src, None, "Twitch", "video"));
        }
        if let Some(chan) = segs.first() {
            let src = format!("https://player.twitch.tv/?channel={chan}&parent={TWITCH_PARENT}&autoplay=true");
            return Some(embed(&src, "twitch", "iframe", &src, None, "Twitch", "video"));
        }
    }
    // Spotify (Media Party 11.4: the localised /intl-xx/ prefix is skipped)
    if host == "open.spotify.com" {
        let sp: &[String] = if segs.first().map(|s| s.starts_with("intl-")).unwrap_or(false) { &segs[1..] } else { &segs[..] };
        if let [kind, id, ..] = sp {
            if matches!(kind.as_str(), "track" | "album" | "playlist" | "episode" | "show" | "artist") {
                let src = format!("https://open.spotify.com/embed/{kind}/{id}");
                let layout = if matches!(kind.as_str(), "album" | "playlist" | "show" | "artist") { "audioLg" } else { "audioSm" };
                return Some(embed(&src, "spotify", "iframe", &src, None, "Spotify", layout));
            }
        }
    }
    // Apple Music (Media Party 11.4): embed.music.apple.com plays 30 s previews (full songs need a subscription in
    // Apple's own player); /<cc>/album/<slug>/<id>[?i=<track>] · /<cc>/song/<slug>/<id> · /<cc>/playlist/<slug>/pl.<id>
    if host == "music.apple.com" || host == "geo.music.apple.com" {
        let has_cc = segs.first().map(|s| s.len() == 2 && s.bytes().all(|b| b.is_ascii_alphabetic())).unwrap_or(false);
        let cc = if has_cc { segs[0].to_ascii_lowercase() } else { "us".to_string() };
        let p: &[String] = if has_cc { &segs[1..] } else { &segs[..] };
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        if let [kind, slug, id, ..] = p {
            if kind == "album" && digits(id) {
                let track = query(&u, "i").filter(|i| digits(i));
                let src = match &track {
                    Some(t) => format!("https://embed.music.apple.com/{cc}/album/{slug}/{id}?i={t}"),
                    None => format!("https://embed.music.apple.com/{cc}/album/{slug}/{id}"),
                };
                return Some(embed(&src, "apple", "iframe", &src, None, "Apple Music", if track.is_some() { "audioSm" } else { "audioLg" }));
            }
            if kind == "song" && digits(id) {
                let src = format!("https://embed.music.apple.com/{cc}/song/{slug}/{id}");
                return Some(embed(&src, "apple", "iframe", &src, None, "Apple Music", "audioSm"));
            }
            if kind == "playlist" && id.strip_prefix("pl.").map(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_alphanumeric())).unwrap_or(false) {
                let src = format!("https://embed.music.apple.com/{cc}/playlist/{slug}/{id}");
                return Some(embed(&src, "apple", "iframe", &src, None, "Apple Music", "audioLg"));
            }
        }
    }
    // SoundCloud
    if host == "soundcloud.com" {
        if segs.len() >= 2 && segs[0] != "discover" && segs[0] != "you" {
            let canonical = format!("https://soundcloud.com/{}", segs.join("/"));
            let src = format!(
                "https://w.soundcloud.com/player/?url={}&color=%23ff5500&auto_play=true&hide_related=true&show_comments=false&show_artwork=true",
                enc(&canonical)
            );
            return Some(embed(&src, "soundcloud", "iframe", &src, None, "SoundCloud", "audioSm"));
        }
    }
    // Direct media
    let path_lower = u.path().to_lowercase();
    if path_lower.ends_with(".mp4") || path_lower.ends_with(".webm") || path_lower.ends_with(".mov") {
        return Some(embed(raw, "media", "video", raw, None, "Video", "video"));
    }
    if path_lower.ends_with(".gifv") {
        let mp4 = format!("{}.mp4", &raw[..raw.len() - 5]);
        return Some(embed(&mp4, "media", "video", &mp4, None, "Video", "video"));
    }
    if path_lower.ends_with(".gif") {
        // M9 (security review 2026-10-26): a GIF shows by itself only from Giphy (the GIF picker's own provider, whose
        // picks arrive as plain links); from any other host it waits for a click (ThEmbedCard), so a stranger's link
        // can't make every reader's client fetch their file (address + time of reading) and decode their bytes.
        let mut e = embed(raw, "media", "image", raw, None, "GIF", "gif");
        e["autoload"] = Value::from(is_giphy_host(&host));
        e["host"] = Value::from(u.host_str().unwrap_or("").to_lowercase());
        return Some(e);
    }
    None
}

/// Giphy's own hosts (giphy.com and its media subdomains: media*.giphy.com, i.giphy.com).
fn is_giphy_host(host: &str) -> bool {
    host == "giphy.com" || host.ends_with(".giphy.com")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(raw: &str) -> Value {
        parse_embed(raw).unwrap_or(Value::Null)
    }
    fn src(v: &Value) -> String {
        v["src"].as_str().unwrap_or("").to_string()
    }

    #[test]
    fn youtube_variants() {
        assert!(src(&e("https://youtu.be/dQw4w9WgXcQ")).contains("youtube-nocookie.com/embed/dQw4w9WgXcQ?autoplay=1&rel=0"));
        assert_eq!(e("https://youtu.be/dQw4w9WgXcQ")["poster"], Value::from("https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg"));
        assert!(src(&e("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=1h2m3s")).ends_with("&start=3723"));
        assert!(src(&e("https://youtube.com/watch?v=dQw4w9WgXcQ&t=90")).ends_with("&start=90"));
        for path in ["shorts/dQw4w9WgXcQ", "embed/dQw4w9WgXcQ", "live/dQw4w9WgXcQ"] {
            assert_eq!(e(&format!("https://www.youtube.com/{path}"))["platform"], Value::from("youtube"));
        }
    }

    #[test]
    fn vimeo_streamable() {
        assert_eq!(src(&e("https://vimeo.com/123456")), "https://player.vimeo.com/video/123456?autoplay=1");
        assert_eq!(src(&e("https://streamable.com/abc123")), "https://streamable.com/e/abc123?autoplay=1");
        assert!(parse_embed("https://streamable.com/e/x").is_none());
    }

    #[test]
    fn twitch() {
        assert!(src(&e("https://clips.twitch.tv/FunnySlug")).contains("clips.twitch.tv/embed?clip=FunnySlug&parent=localhost"));
        assert!(src(&e("https://twitch.tv/somechan/clip/FunnySlug")).contains("clip=FunnySlug&parent=localhost"));
        assert!(src(&e("https://twitch.tv/videos/42")).contains("player.twitch.tv/?video=42&parent=localhost"));
        assert!(src(&e("https://twitch.tv/somechan")).contains("?channel=somechan&parent=localhost"));
    }

    /// Media Party 11.4 (the web's embeds.ts has the same cases): YouTube Music, Spotify intl, Apple Music.
    #[test]
    fn music_services() {
        let ym = e("https://music.youtube.com/watch?v=dQw4w9WgXcQ&list=RDAMVMdQw4w9WgXcQ");
        assert_eq!(ym["platform"], Value::from("youtube"));
        assert_eq!(ym["label"], Value::from("YouTube Music"));
        assert!(src(&ym).contains("/embed/dQw4w9WgXcQ?"));
        assert_eq!(src(&e("https://open.spotify.com/intl-de/track/4cOdK2wGLETKBW3PvgPWqT?si=x")), "https://open.spotify.com/embed/track/4cOdK2wGLETKBW3PvgPWqT");
        let t = e("https://music.apple.com/us/album/never-gonna-give-you-up/1440857781?i=1440857901");
        assert_eq!(src(&t), "https://embed.music.apple.com/us/album/never-gonna-give-you-up/1440857781?i=1440857901");
        assert_eq!(t["layout"], Value::from("audioSm"));
        assert_eq!(t["label"], Value::from("Apple Music"));
        assert_eq!(e("https://music.apple.com/gb/album/x/1440857781")["layout"], Value::from("audioLg"));
        assert_eq!(src(&e("https://music.apple.com/us/song/never/1440857901")), "https://embed.music.apple.com/us/song/never/1440857901");
        assert_eq!(src(&e("https://music.apple.com/us/playlist/todays-hits/pl.f4d106fed2bd41149aaacabb233eb5eb")), "https://embed.music.apple.com/us/playlist/todays-hits/pl.f4d106fed2bd41149aaacabb233eb5eb");
        assert_eq!(src(&e("https://music.apple.com/album/x/1440857781")), "https://embed.music.apple.com/us/album/x/1440857781");
        assert!(parse_embed("https://music.apple.com/us/artist/rick-astley/669771").is_none());
    }

    #[test]
    fn spotify_soundcloud() {
        assert_eq!(src(&e("https://open.spotify.com/track/abc")), "https://open.spotify.com/embed/track/abc");
        assert_eq!(e("https://open.spotify.com/track/abc")["layout"], Value::from("audioSm"));
        assert_eq!(e("https://open.spotify.com/album/abc")["layout"], Value::from("audioLg"));
        assert!(src(&e("https://soundcloud.com/user/track")).contains("w.soundcloud.com/player/?url=https%3A%2F%2Fsoundcloud.com%2Fuser%2Ftrack"));
        assert!(parse_embed("https://soundcloud.com/discover").is_none());
    }

    #[test]
    fn direct_media() {
        assert_eq!(e("https://x.com/a.mp4")["mode"], Value::from("video"));
        assert_eq!(src(&e("https://x.com/a.gifv")), "https://x.com/a.mp4");
        assert_eq!(e("https://x.com/a.gif")["mode"], Value::from("image"));
        // M9: only Giphy's GIFs show by themselves; any other host's wait for a click
        assert_eq!(e("https://x.com/a.gif")["autoload"], Value::from(false));
        assert_eq!(e("https://x.com/a.gif")["host"], Value::from("x.com"));
        assert_eq!(e("https://media4.giphy.com/media/abc/giphy.gif")["autoload"], Value::from(true));
        assert_eq!(e("https://i.giphy.com/abc.gif")["autoload"], Value::from(true));
        assert_eq!(e("https://giphy.com/x.gif")["autoload"], Value::from(true));
        assert_eq!(e("https://notgiphy.com/x.gif")["autoload"], Value::from(false));
        assert_eq!(e("https://giphy.com.evil.example/x.gif")["autoload"], Value::from(false));
    }

    #[test]
    fn extract_dedupe_cap_punct_reply() {
        // trailing punctuation trimmed
        assert_eq!(extract_embeds("see https://youtu.be/dQw4w9WgXcQ).").len(), 1);
        // dedupe same URL twice -> one
        assert_eq!(extract_embeds("https://x.com/a.gif https://x.com/a.gif").len(), 1);
        // cap at 3
        let many = "https://x.com/1.gif https://x.com/2.gif https://x.com/3.gif https://x.com/4.gif";
        assert_eq!(extract_embeds(many).len(), 3);
        // reply fallback stripped, real URL still parsed
        assert_eq!(extract_embeds("> quoted\n\nhttps://youtu.be/dQw4w9WgXcQ").len(), 1);
    }

    #[test]
    fn only_embed() {
        assert!(body_is_only_embed("https://x.com/a.gif"));
        assert!(!body_is_only_embed("https://youtu.be/dQw4w9WgXcQ")); // iframe keeps URL
        assert!(!body_is_only_embed("hey https://x.com/a.gif")); // whitespace
    }

    /// The shipped themes paint the spoiler exactly as 10.6 did (#9a9a9a tag, #5a5a5a block).
    #[test]
    fn builtin_themes_keep_the_spoiler_colours() {
        for id in ["oled", "goose"] {
            let a = crate::theme_rt::try_resolve_in(id, std::path::Path::new("/nonexistent")).unwrap();
            let hex = |t: &str| crate::theme_rt::qt_string(&a.colour(t).unwrap());
            assert_eq!(hex("bubble.spoiler.tag.fg"), "#9a9a9a", "{id}");
            assert_eq!(hex("bubble.spoiler.bg"), "#5a5a5a", "{id}");
        }
    }

    #[test]
    fn spoilers_are_hidden_not_shown() {
        // 10.1 (TH-MSG-05)
        let plain = "<b>no spoiler here</b>";
        assert_eq!(hide_spoilers(plain), plain);
        let s = hide_spoilers("before <span data-mx-spoiler>the butler did it</span> after");
        assert!(s.contains("[spoiler — select to reveal]"), "{s}");
        let bg = crate::theme_rt::qt_colour("bubble.spoiler.bg");
        assert!(s.contains(&format!("background-color:{bg};color:{bg}\">the butler did it</span>")), "{s}");
        assert!(!s.contains("data-mx-spoiler"));
        let r = hide_spoilers("<span data-mx-spoiler=\"Valheim ending\">x</span>");
        assert!(r.contains("[spoiler: Valheim ending — select to reveal]"), "{r}");
    }
}
