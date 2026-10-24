//! Link previews for ordinary links (9.3).
//!
//! A pasted article / store page / repo link gets a card (site, title, description, image)
//! the way it does on Discord, Slack and Element. The homeserver does the spidering:
//! `/_matrix/client/v1/media/preview_url` (Synapse `url_preview_enabled`) fetches the
//! page from the SERVER's address and returns OpenGraph data; the og:image is re-hosted
//! as an mxc:// which we download into our cache. The app itself never touches the
//! third-party site, so a link in chat leaks nothing about who is reading it — the same
//! privacy model as the web client.
//!
//! Cache: memory for the session + on disk (`$XDG_CACHE_HOME/Tin Hat/linkpreview`).
//! Positive entries live 7 days, "no preview" 6 hours, so a room reload never re-fetches
//! and a page that had no metadata yesterday is retried tomorrow. Fetches never sit on the
//! timeline load path — `room.rs` attaches what is cached and back-fills the rest.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

const POSITIVE_TTL_SECS: u64 = 7 * 24 * 3600;
const NEGATIVE_TTL_SECS: u64 = 6 * 3600;
/// og:image larger than this is not downloaded (Synapse reports `matrix:image:size`).
const IMAGE_MAX_BYTES: u64 = 4 * 1024 * 1024;
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);
const IMAGE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);

#[derive(Clone)]
struct Entry {
    at: u64,
    value: Option<Value>,
}

fn mem() -> &'static Mutex<HashMap<String, Entry>> {
    static M: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Sign-out (signout.rs, which removes cache_dir): the previews this session fetched.
pub(crate) fn forget_session() {
    mem().lock().unwrap_or_else(|p| p.into_inner()).clear();
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `$XDG_CACHE_HOME/Tin Hat/linkpreview` (swept by room.rs' cache sweep).
pub(crate) fn cache_dir() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache")
        });
    base.join("TinHat/linkpreview")
}

fn key_of(url: &str) -> String {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(url.as_bytes());
    d.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

fn fresh(e: &Entry) -> bool {
    let ttl = if e.value.is_some() { POSITIVE_TTL_SECS } else { NEGATIVE_TTL_SECS };
    now_secs().saturating_sub(e.at) < ttl
}

/// What we know about `url` without any network: `Some(Some(card))` = a preview,
/// `Some(None)` = known to have none (recently), `None` = never looked (or expired).
pub fn cached(url: &str) -> Option<Option<Value>> {
    if let Some(e) = mem().lock().unwrap().get(url) {
        if fresh(e) {
            return Some(e.value.clone());
        }
    }
    let path = cache_dir().join(format!("{}.json", key_of(url)));
    let raw = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    let at = v.get("at").and_then(|a| a.as_u64()).unwrap_or(0);
    let value = v.get("value").filter(|x| !x.is_null()).cloned();
    let e = Entry { at, value };
    if !fresh(&e) {
        return None;
    }
    let out = e.value.clone();
    mem().lock().unwrap().insert(url.to_string(), e);
    Some(out)
}

fn store(url: &str, value: Option<Value>) {
    let e = Entry { at: now_secs(), value: value.clone() };
    mem().lock().unwrap().insert(url.to_string(), e);
    let dir = cache_dir();
    let path = dir.join(format!("{}.json", key_of(url)));
    let doc = serde_json::json!({ "at": now_secs(), "url": url, "value": value }).to_string();
    let _ = crate::bridge::runtime().spawn_blocking(move || {
        if std::fs::create_dir_all(&dir).is_ok() {
            let _ = std::fs::write(path, doc);
        }
    });
}

/// The first URL in a message body worth a preview card: http(s), a real host name (not
/// an IP / bare word), not `matrix.to`, not one of the inline media embeds (YouTube, Twitch,
/// Spotify, a direct .mp4/.gif — those already render as their own card), not a plain
/// file link.
pub fn previewable_url(body: &str) -> Option<String> {
    for m in crate::embeds::url_re().find_iter(body) {
        let raw = m.as_str().trim_end_matches(|c: char| ")].,!?;:'\"".contains(c));
        let Ok(u) = url::Url::parse(raw) else { continue };
        if u.scheme() != "http" && u.scheme() != "https" {
            continue;
        }
        let host = u.host_str().unwrap_or("").to_lowercase();
        if host.is_empty() || host == "matrix.to" || host.ends_with(".matrix.to") {
            continue;
        }
        if host.parse::<std::net::IpAddr>().is_ok() || !host.contains('.') {
            continue;
        }
        if crate::embeds::parse_embed(raw).is_some() {
            continue;
        }
        let p = u.path().to_lowercase();
        const SKIP: [&str; 14] = [
            ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".mp4", ".webm", ".mp3", ".ogg",
            ".pdf", ".zip", ".7z", ".exe",
        ];
        if SKIP.iter().any(|e| p.ends_with(e)) {
            continue;
        }
        return Some(raw.to_string());
    }
    None
}

/// The preview for `url`, fetching (and caching either outcome) when unknown.
pub async fn fetch(url: &str) -> Option<Value> {
    if let Some(c) = cached(url) {
        return c;
    }
    let v = fetch_uncached(url).await;
    store(url, v.clone());
    v
}

fn og_str(data: &Value, key: &str, max: usize) -> String {
    let s = data.get(key).and_then(|v| v.as_str()).unwrap_or("");
    let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > max {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    } else {
        s
    }
}

fn og_num(data: &Value, key: &str) -> u64 {
    match data.get(key) {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse::<u64>().unwrap_or(0),
        _ => 0,
    }
}

async fn fetch_uncached(url: &str) -> Option<Value> {
    use matrix_sdk::ruma::api::client::authenticated_media::get_media_preview::v1::Request;
    let client = crate::session::client()?;
    let req = Request::new(url.to_string());
    let resp = match tokio::time::timeout(FETCH_TIMEOUT, client.send(req)).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            eprintln!("[preview] {url}: {e}");
            return None;
        }
        Err(_) => {
            eprintln!("[preview] {url}: timed out");
            return None;
        }
    };
    let data: Value = resp.data.and_then(|d| serde_json::from_str(d.get()).ok())?;
    let title = og_str(&data, "og:title", 160);
    let description = og_str(&data, "og:description", 320);
    let site = og_str(&data, "og:site_name", 60);
    let image_mxc = data.get("og:image").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if title.is_empty() && description.is_empty() && image_mxc.is_empty() {
        return None;
    }
    let img_size = og_num(&data, "matrix:image:size");
    let mut image = String::new();
    let (mut iw, mut ih) = (og_num(&data, "og:image:width"), og_num(&data, "og:image:height"));
    if image_mxc.starts_with("mxc://") && (img_size == 0 || img_size <= IMAGE_MAX_BYTES) {
        image = download_image(&client, &image_mxc).await.unwrap_or_default();
    }
    if image.is_empty() {
        iw = 0;
        ih = 0;
    }
    let host = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.trim_start_matches("www.").to_string()))
        .unwrap_or_default();
    Some(serde_json::json!({
        "url": url,
        "title": title,
        "description": description,
        "siteName": site,
        "host": host,
        "image": image,
        "imageW": iw,
        "imageH": ih,
    }))
}

async fn download_image(client: &matrix_sdk::Client, mxc: &str) -> Option<String> {
    use matrix_sdk::media::{MediaFormat, MediaRequestParameters};
    use matrix_sdk::ruma::events::room::MediaSource;
    use matrix_sdk::ruma::OwnedMxcUri;
    let dir = cache_dir();
    let path = dir.join(format!("{}.img", key_of(mxc)));
    if path.exists() {
        return Some(crate::platform::file_url(&path));
    }
    let uri = OwnedMxcUri::from(mxc.to_string());
    let req = MediaRequestParameters { source: MediaSource::Plain(uri), format: MediaFormat::File };
    let bytes = tokio::time::timeout(IMAGE_TIMEOUT, client.media().get_media_content(&req, true))
        .await
        .ok()?
        .ok()?;
    if bytes.is_empty() {
        return None;
    }
    tokio::fs::create_dir_all(&dir).await.ok()?;
    tokio::fs::write(&path, &bytes).await.ok()?;
    Some(crate::platform::file_url(&path))
}

#[cfg(test)]
mod tests {
    use super::previewable_url;

    #[test]
    fn picks_a_plain_page_link_and_skips_embeds_media_and_matrix_to() {
        assert_eq!(
            previewable_url("look https://store.steampowered.com/app/892970/Valheim/ wow"),
            Some("https://store.steampowered.com/app/892970/Valheim/".to_string())
        );
        assert_eq!(previewable_url("https://youtu.be/dQw4w9WgXcQ"), None);
        assert_eq!(previewable_url("https://i.imgur.com/abc.gif"), None);
        assert_eq!(previewable_url("https://cdn.example.com/photo.jpg"), None);
        assert_eq!(previewable_url("https://matrix.to/#/@blake:example.com"), None);
        assert_eq!(previewable_url("http://192.0.2.15/"), None);
        assert_eq!(previewable_url("no links here"), None);
    }

    #[test]
    fn trailing_punctuation_is_not_part_of_the_link() {
        assert_eq!(
            previewable_url("(see https://en.wikipedia.org/wiki/Valheim)."),
            Some("https://en.wikipedia.org/wiki/Valheim".to_string())
        );
    }
}
