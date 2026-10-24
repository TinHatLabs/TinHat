//! Media Party link parsing (11.4, DESIGN §2.5 / CONTRACT §3) — the byte-compatible twin of the web's
//! `src/features/media/watchparty/links.ts`. PURE: no Matrix, no network. `parse_media_link(text)` classifies what a
//! person pasted or a chat card carries into the same JSON shape the web produces (`{kind, input, service?, id?,
//! entity?, playUrl?, list?, start?, art?, handle?}`):
//!   youtube   www./m./youtube.com/watch?v= · youtu.be/<id> · /shorts/ /live/ /embed/ /v/ · youtube-nocookie.com
//!   ytmusic   music.youtube.com/watch?v= (a song — plays as the YouTube video) · music.youtube.com/playlist?list=
//!   playlist  youtube.com/playlist?list= (a watch url's &list= rides along as `list`)
//!   channel   youtube.com/@handle · /channel/<id> · /c/<name> · /user/<name>
//!   spotify   open.spotify.com/(intl-xx/)?(track|album|playlist)/<id> · spotify:track:<id>
//!   apple     music.apple.com/<cc>/album/<slug>/<albumId>?i=<trackId> · /song/<slug>/<id> · /playlist/<slug>/pl.<id>
//!   file      a direct media url (mp4/webm/ogv/ogg/mov/mp3/m4a/opus/wav/flac/m3u8)
//!   link      any other http(s) url (mpv + yt-dlp may still play it)
//!   text      not a url → a search query
//! The shared vectors live in `testvectors/mediaparty/links.json` (both repos): every URL shape → this exact result.

use serde_json::{json, Map, Value};
use url::Url;

fn is_yt_id(s: &str) -> bool {
    (6..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn is_sp_id(s: &str) -> bool {
    (6..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric())
}
fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn query<'a>(u: &'a Url, key: &str) -> Option<String> {
    u.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned())
}

fn parse_start(t: Option<String>) -> u64 {
    let Some(t) = t else { return 0 };
    if is_digits(&t) {
        return t.parse().unwrap_or(0);
    }
    // (\d+h)?(\d+m)?(\d+s)?
    let (mut total, mut cur, mut any) = (0u64, 0u64, false);
    for ch in t.chars() {
        if ch.is_ascii_digit() {
            cur = cur * 10 + (ch as u64 - '0' as u64);
        } else {
            let mult = match ch {
                'h' => 3600,
                'm' => 60,
                's' => 1,
                _ => return 0,
            };
            total += cur * mult;
            cur = 0;
            any = true;
        }
    }
    if any && cur == 0 { total } else { 0 }
}

fn media_ext(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    ["mp4", "m4v", "webm", "ogv", "ogg", "mov", "mp3", "m4a", "opus", "wav", "flac", "m3u8"]
        .iter()
        .any(|e| p.ends_with(&format!(".{e}")))
}

fn link(kind: &str, input: &str) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("kind".into(), Value::from(kind));
    m.insert("input".into(), Value::from(input));
    m
}

fn yt_video(input: &str, id: &str, u: &Url, service: &str) -> Value {
    let mut m = link(if service == "ytmusic" { "ytmusic" } else { "youtube" }, input);
    m.insert("service".into(), Value::from(service));
    m.insert("id".into(), Value::from(id));
    m.insert("entity".into(), Value::from("track"));
    m.insert("playUrl".into(), Value::from(format!("https://www.youtube.com/watch?v={id}")));
    m.insert("art".into(), Value::from(format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg")));
    let start = parse_start(query(u, "t").or_else(|| query(u, "start")));
    if start > 0 {
        m.insert("start".into(), Value::from(start));
    }
    if let Some(list) = query(u, "list").filter(|l| is_yt_id(l)) {
        m.insert("list".into(), Value::from(list));
    }
    Value::Object(m)
}

fn plain(kind: &str, input: &str) -> Value {
    let mut m = link(kind, input);
    m.insert("playUrl".into(), Value::from(input));
    Value::Object(m)
}

/// Classify `text` (see the module doc). Always returns an object with `kind` + `input`.
pub fn parse_media_link(text: &str) -> Value {
    let input = text.trim();
    if input.is_empty() {
        return Value::Object(link("text", input));
    }
    // spotify:track:<id>
    if let Some(rest) = input.strip_prefix("spotify:") {
        let parts: Vec<&str> = rest.split(':').collect();
        if parts.len() == 2 && matches!(parts[0], "track" | "album" | "playlist") && is_sp_id(parts[1]) {
            let mut m = link("spotify", input);
            m.insert("service".into(), Value::from("spotify"));
            m.insert("id".into(), Value::from(parts[1]));
            m.insert("entity".into(), Value::from(parts[0]));
            m.insert("playUrl".into(), Value::from(format!("https://open.spotify.com/{}/{}", parts[0], parts[1])));
            return Value::Object(m);
        }
    }
    let lower = input.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) || input.chars().any(char::is_whitespace) {
        return Value::Object(link("text", input));
    }
    let Ok(u) = Url::parse(input) else { return Value::Object(link("text", input)) };
    let host = u.host_str().unwrap_or("").to_ascii_lowercase();
    let bare = host.strip_prefix("www.").or_else(|| host.strip_prefix("m.")).unwrap_or(&host).to_string();
    let segs: Vec<String> = u
        .path_segments()
        .map(|s| s.filter(|p| !p.is_empty()).map(str::to_string).collect())
        .unwrap_or_default();

    // ---- YouTube / YouTube Music ----
    if bare == "youtu.be" {
        if let Some(id) = segs.first().filter(|s| is_yt_id(s)) {
            return yt_video(input, id, &u, "youtube");
        }
        return plain("link", input);
    }
    if bare == "youtube.com" || bare == "youtube-nocookie.com" || host == "music.youtube.com" {
        let service = if host == "music.youtube.com" { "ytmusic" } else { "youtube" };
        if u.path() == "/watch" {
            if let Some(v) = query(&u, "v").filter(|v| is_yt_id(v)) {
                return yt_video(input, &v, &u, service);
            }
        }
        if u.path() == "/playlist" {
            if let Some(list) = query(&u, "list").filter(|l| is_yt_id(l)) {
                let mut m = link("playlist", input);
                m.insert("service".into(), Value::from(service));
                m.insert("id".into(), Value::from(list.as_str()));
                m.insert("entity".into(), Value::from("playlist"));
                m.insert("playUrl".into(), Value::from(format!("https://www.youtube.com/playlist?list={list}")));
                return Value::Object(m);
            }
        }
        if segs.len() >= 2 && matches!(segs[0].as_str(), "shorts" | "live" | "embed" | "v") && is_yt_id(&segs[1]) {
            return yt_video(input, &segs[1], &u, service);
        }
        if let Some(h) = segs.first().filter(|s| s.starts_with('@') && s.len() > 1) {
            let mut m = link("channel", input);
            m.insert("service".into(), Value::from("youtube"));
            m.insert("handle".into(), Value::from(h.as_str()));
            m.insert("id".into(), Value::from(h.as_str()));
            m.insert("playUrl".into(), Value::from(format!("https://www.youtube.com/{h}")));
            return Value::Object(m);
        }
        if segs.len() >= 2 && segs[0] == "channel" && is_yt_id(&segs[1]) {
            let mut m = link("channel", input);
            m.insert("service".into(), Value::from("youtube"));
            m.insert("id".into(), Value::from(segs[1].as_str()));
            m.insert("playUrl".into(), Value::from(format!("https://www.youtube.com/channel/{}", segs[1])));
            return Value::Object(m);
        }
        if segs.len() >= 2 && (segs[0] == "c" || segs[0] == "user") {
            let mut m = link("channel", input);
            m.insert("service".into(), Value::from("youtube"));
            m.insert("id".into(), Value::from(segs[1].as_str()));
            m.insert("playUrl".into(), Value::from(format!("https://www.youtube.com/{}/{}", segs[0], segs[1])));
            return Value::Object(m);
        }
        return plain("link", input);
    }

    // ---- Spotify ----
    if bare == "open.spotify.com" {
        let p: &[String] = if segs.first().map(|s| s.starts_with("intl-")).unwrap_or(false) { &segs[1..] } else { &segs[..] };
        if p.len() >= 2 && matches!(p[0].as_str(), "track" | "album" | "playlist") && is_sp_id(&p[1]) {
            let mut m = link("spotify", input);
            m.insert("service".into(), Value::from("spotify"));
            m.insert("id".into(), Value::from(p[1].as_str()));
            m.insert("entity".into(), Value::from(p[0].as_str()));
            m.insert("playUrl".into(), Value::from(format!("https://open.spotify.com/{}/{}", p[0], p[1])));
            return Value::Object(m);
        }
        return plain("link", input);
    }

    // ---- Apple Music ----
    if bare == "music.apple.com" || bare == "geo.music.apple.com" {
        let p: &[String] = if segs.first().map(|s| s.len() == 2 && s.bytes().all(|b| b.is_ascii_alphabetic())).unwrap_or(false) {
            &segs[1..]
        } else {
            &segs[..]
        };
        let last = p.last().map(String::as_str).unwrap_or("");
        let apple = |id: &str, entity: &str| {
            let mut m = link("apple", input);
            m.insert("service".into(), Value::from("apple"));
            m.insert("id".into(), Value::from(id));
            m.insert("entity".into(), Value::from(entity));
            m.insert("playUrl".into(), Value::from(input));
            Value::Object(m)
        };
        if p.first().map(String::as_str) == Some("album") && is_digits(last) {
            if let Some(i) = query(&u, "i").filter(|i| is_digits(i)) {
                return apple(&i, "track");
            }
            return apple(last, "album");
        }
        if p.first().map(String::as_str) == Some("song") && is_digits(last) {
            return apple(last, "track");
        }
        if p.first().map(String::as_str) == Some("playlist")
            && last.strip_prefix("pl.").map(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_alphanumeric())).unwrap_or(false)
        {
            return apple(last, "playlist");
        }
        return plain("link", input);
    }

    if media_ext(u.path()) {
        return plain("file", input);
    }
    plain("link", input)
}

/// The badge a queue item / card shows for a service ("" when none).
pub fn service_label(service: &str, kind: &str) -> &'static str {
    match service {
        "spotify" => "Spotify",
        "apple" => "Apple Music",
        "ytmusic" => "YouTube Music",
        "youtube" => "YouTube",
        _ => match kind {
            "file" => "File",
            "song" => "Song",
            _ => "",
        },
    }
}

/// The YouTube video id of a playing url (youtube / ytmusic), else "".
pub fn youtube_id_of(url: &str) -> String {
    let v = parse_media_link(url);
    match v.get("kind").and_then(Value::as_str) {
        Some("youtube") | Some("ytmusic") => v.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(s: &str) -> String {
        parse_media_link(s)["kind"].as_str().unwrap().to_string()
    }

    /// The shared vectors (testvectors/mediaparty/links.json, the web runs the same file): every URL shape → the same
    /// object, byte for byte after canonical serialisation.
    #[test]
    fn shared_vectors() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/testvectors/mediaparty/links.json");
        let text = std::fs::read_to_string(path).expect("testvectors/mediaparty/links.json");
        let v: Value = serde_json::from_str(&text).unwrap();
        let cases = v["cases"].as_array().unwrap();
        assert!(cases.len() >= 30);
        for c in cases {
            let input = c["input"].as_str().unwrap();
            let got = parse_media_link(input);
            assert_eq!(got, c["expect"], "vector {:?} ({})", c["name"], input);
        }
    }

    #[test]
    fn kinds() {
        assert_eq!(k("https://www.youtube.com/watch?v=dQw4w9WgXcQ"), "youtube");
        assert_eq!(k("https://music.youtube.com/watch?v=dQw4w9WgXcQ"), "ytmusic");
        assert_eq!(k("https://open.spotify.com/intl-de/track/4cOdK2wGLETKBW3PvgPWqT"), "spotify");
        assert_eq!(k("spotify:track:4cOdK2wGLETKBW3PvgPWqT"), "spotify");
        assert_eq!(k("https://music.apple.com/us/album/x/1440857781?i=1440857901"), "apple");
        assert_eq!(k("https://x.com/a.mp4"), "file");
        assert_eq!(k("https://example.com/page"), "link");
        assert_eq!(k("lofi hip hop"), "text");
        assert_eq!(k(""), "text");
        assert_eq!(youtube_id_of("https://youtu.be/dQw4w9WgXcQ?t=1m2s"), "dQw4w9WgXcQ");
        assert_eq!(parse_media_link("https://youtu.be/dQw4w9WgXcQ?t=1m2s")["start"], Value::from(62));
        assert_eq!(service_label("apple", ""), "Apple Music");
    }
}
