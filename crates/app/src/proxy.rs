//! The Tin Hat API client — shared infra (Phase 3). A single tiny helper hitting the user's own server's API
//! (`<api>` = the base its `.well-known/matrix/client` announces, selfhost: server_offer.rs) so third-party API
//! keys (Giphy / myinstants / YouTube / Steam) stay server-side and never ship in the client. Consumed by the gif
//! picker, soundboard, watch-party search, Game Reel, Memories, call moderation, invites, Reach presence, the Game
//! servers widget, problem reports and the speed test. Uses the reqwest already in the tree via matrix-sdk (no new
//! dependency).
//!
//! selfhost (2026-10-21: nothing server-specific hard-coded in the app): the app has no API host of its own.
//! [`base`] is, in order: `TINHAT_PROXY_BASE` (a test override), the `api` the signed-in server announced
//! ([`crate::server_offer::api_base`]), else `<homeserver origin>/api` (a Tin Hat Server serves its API there);
//! signed out, there is no base and every call fails with a plain sentence. URLs the server handed out under a name
//! it had BEFORE it moved to its own domain (uploaded-sound links, Memories signed file links) are mapped onto today's
//! base before they are fetched: [`rebase_url`] over [`known_bases`] = the current base + `https://<former>/api` for
//! the names the server's own well-known lists (`dev.tinhat.moved_from`, server_offer.rs). Nothing else is trusted:
//! no list of every server this device ever saw (Astra review 2026-10-21 — that re-homed one server's signed links
//! onto another's after an account switch).
//!
//! 10.5.2: every request to the API carries `User-Agent: TinHat/<version> (<os>)` ([`user_agent`], [`http`]) so a
//! server's logs tell desktop versions apart. reqwest sends no User-Agent by default. Third-party fetches (YouTube
//! oEmbed, link previews, …) keep their own agent.

/// Base URL for the API (no trailing slash): `TINHAT_PROXY_BASE`, else the server's announced `api`, else
/// `<homeserver origin>/api`; "" when signed out (and nothing overrides).
pub fn base() -> String {
    base_from(std::env::var("TINHAT_PROXY_BASE").ok(), crate::server_offer::api_base(), crate::session::server_origin())
}

fn base_from(env: Option<String>, announced: Option<String>, origin: Option<String>) -> String {
    let clean = |s: String| s.trim().trim_end_matches('/').to_string();
    env.map(clean)
        .filter(|s| !s.is_empty())
        .or_else(|| announced.map(clean).filter(|s| !s.is_empty()))
        .or_else(|| origin.map(clean).filter(|s| !s.is_empty()).map(|o| format!("{o}/api")))
        .unwrap_or_default()
}

/// The bases a link of this account's server may sit under, the current one first: today's base, then
/// `https://<former name>/api` for every name the server had before it moved (its well-known says so).
pub fn known_bases() -> Vec<String> {
    known_from(&base(), &crate::server_offer::former_names())
}

fn known_from(cur: &str, former: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    if cur.is_empty() {
        return out;
    }
    out.push(cur.to_string());
    for n in former {
        let b = format!("https://{n}/api");
        if !out.iter().any(|o| o.eq_ignore_ascii_case(&b)) {
            out.push(b);
        }
    }
    out
}

/// A URL on a former base of this server ([`known_bases`]) re-homed onto the current [`base`]; any other URL
/// (myinstants, Giphy, mxc://, a relative path, …) comes back unchanged.
pub fn rebase_url(url: &str) -> String {
    let b = base();
    if b.is_empty() {
        return url.to_string();
    }
    rebase_onto(url, &b, &known_bases())
}

fn rebase_onto(url: &str, base: &str, known: &[String]) -> String {
    for k in known {
        let Some(head) = url.get(..k.len()) else { continue };
        if !head.eq_ignore_ascii_case(k) {
            continue;
        }
        let rest = &url[k.len()..];
        // only a whole path segment: `/api/...`, `/api?...`, `/api` — never `/apixyz`
        if rest.is_empty() || rest.starts_with('/') || rest.starts_with('?') || rest.starts_with('#') {
            return format!("{base}{rest}");
        }
    }
    url.to_string()
}

/// [`rebase_url`] on every string inside a JSON value (the Memories list carries its
/// signed file links in `tracks[].url`, `tracks[].combined_url`, `mixed`, …).
pub fn rebase_urls_in_json(v: &mut serde_json::Value) {
    let b = base();
    if b.is_empty() {
        return;
    }
    let known = known_bases();
    rebase_json_onto(v, &b, &known);
}

fn rebase_json_onto(v: &mut serde_json::Value, base: &str, known: &[String]) {
    match v {
        serde_json::Value::String(s) => {
            let r = rebase_onto(s, base, known);
            if r != *s {
                *s = r;
            }
        }
        serde_json::Value::Array(a) => a.iter_mut().for_each(|x| rebase_json_onto(x, base, known)),
        serde_json::Value::Object(o) => o.values_mut().for_each(|x| rebase_json_onto(x, base, known)),
        _ => {}
    }
}

/// `TinHat/<version> (<os>)`, e.g. `TinHat/10.5.2 (linux)` / `TinHat/10.5.2 (windows)` — the
/// User-Agent of every request to our API (the build version the updater compares, and Rust's
/// target OS name).
pub fn user_agent() -> String {
    agent_for(env!("CARGO_PKG_VERSION"), std::env::consts::OS)
}

fn agent_for(version: &str, os: &str) -> String {
    format!("TinHat/{version} ({os})")
}

/// The HTTP client for our API: [`user_agent`] + an optional overall timeout. Every `/api`
/// caller (gifs, soundboard search + upload, watch search, Game Reel catalog, invites, call
/// moderation, the Memories list) builds its client here.
pub fn http(timeout: Option<std::time::Duration>) -> matrix_sdk::reqwest::Client {
    let mut b = matrix_sdk::reqwest::Client::builder().user_agent(user_agent());
    if let Some(t) = timeout {
        b = b.timeout(t);
    }
    b.build().unwrap_or_else(|_| matrix_sdk::reqwest::Client::new())
}

/// What every caller says when there is no API to talk to.
pub const NO_SERVER: &str = "not signed in to a server";

/// `<base><path>`, or the sentence for "no server".
pub fn url(path: &str) -> Result<String, String> {
    let b = base();
    if b.is_empty() {
        return Err(NO_SERVER.to_string());
    }
    Ok(format!("{b}{path}"))
}

/// GET `<base><path>` and parse the JSON body. `path` must start with `/` and carry any
/// (already percent-encoded) query string. Returns the parsed value or a description.
pub async fn get_json(path: &str) -> Result<serde_json::Value, String> {
    let url = url(path)?;
    let resp = http(None)
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("proxy GET {url} failed: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("proxy HTTP {status} for {url}: {text}"));
    }
    serde_json::from_str(&text).map_err(|e| format!("proxy JSON parse failed for {url}: {e}"))
}

/// Like [`get_json`] but with a bearer token. The vods (Memories/DVR) API validates
/// the caller's own Matrix access token and checks room membership server-side, so
/// recordings are only listable/streamable by people who are in the room.
pub async fn get_json_auth(path: &str, bearer: &str) -> Result<serde_json::Value, String> {
    let url = url(path)?;
    let resp = http(None)
        .get(&url)
        .header("Authorization", format!("Bearer {bearer}"))
        .send()
        .await
        .map_err(|e| format!("proxy GET {url} failed: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("proxy HTTP {status} for {url}: {text}"));
    }
    serde_json::from_str(&text).map_err(|e| format!("proxy JSON parse failed for {url}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // sample servers: a Tin Hat Server's own api, and the two names one hub server was served under before
    const MINE: &str = "https://maplehouse.tinhat.me/api";
    const TINHAT: &str = "https://tinhat.example.com/api";
    const ELEMENT: &str = "https://element.example.com/api";

    fn known(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_base_is_the_override_then_the_announced_api_then_the_homeserver_origin() {
        assert_eq!(base_from(None, None, None), "", "signed out: no base");
        assert_eq!(base_from(None, None, Some("https://maplehouse.tinhat.me".into())), MINE);
        assert_eq!(base_from(None, None, Some("https://maplehouse.tinhat.me/".into())), MINE);
        assert_eq!(base_from(None, Some("https://api.maplehouse.tinhat.me/api/".into()), Some("https://maplehouse.tinhat.me".into())), "https://api.maplehouse.tinhat.me/api");
        assert_eq!(base_from(Some("http://127.0.0.1:3001/api/".into()), Some(MINE.into()), Some("https://x".into())), "http://127.0.0.1:3001/api");
        assert_eq!(base_from(Some(String::new()), Some(MINE.into()), None), MINE, "an empty override is no override");
        assert_eq!(base_from(Some("   ".into()), None, Some("https://x.test".into())), "https://x.test/api");
    }

    #[test]
    fn urls_on_an_earlier_base_move_onto_the_current_one() {
        let k = known(&[MINE, ELEMENT, TINHAT]);
        // an uploaded sound as the hub handed it out before its public base moved
        assert_eq!(rebase_onto("https://element.example.com/api/soundboard/clip?id=ab12", MINE, &k), "https://maplehouse.tinhat.me/api/soundboard/clip?id=ab12");
        // a Memories signed file link: path + signature survive byte for byte
        let signed = "https://tinhat.example.com/api/vods/file/_r_x/1789/mixed.mka?exp=1790000000000&sig=deadbeef";
        assert_eq!(rebase_onto(signed, MINE, &k), "https://maplehouse.tinhat.me/api/vods/file/_r_x/1789/mixed.mka?exp=1790000000000&sig=deadbeef");
        assert_eq!(rebase_onto("https://element.example.com/api", MINE, &k), MINE);
        assert_eq!(rebase_onto("HTTPS://Element.Example.com/api/gifs/trending", MINE, &k), "https://maplehouse.tinhat.me/api/gifs/trending");
        // a server that moved to its own domain: links from its old name follow
        let moved = known(&["https://chat.crew.test/api", "https://alpha.local/api"]);
        assert_eq!(rebase_onto("https://alpha.local/api/soundboard/clip?id=1", "https://chat.crew.test/api", &moved), "https://chat.crew.test/api/soundboard/clip?id=1");
        // the current base itself is left as it is
        assert_eq!(rebase_onto("https://maplehouse.tinhat.me/api/x", MINE, &k), "https://maplehouse.tinhat.me/api/x");
        // a base this account never used is not ours
        assert_eq!(rebase_onto("https://other.tinhat.me/api/x", MINE, &k), "https://other.tinhat.me/api/x");
    }

    #[test]
    fn everything_else_is_left_alone() {
        let k = known(&[MINE, ELEMENT]);
        for u in [
            "",
            "https://www.myinstants.com/media/sounds/airhorn.mp3",
            "https://media.giphy.com/media/xyz/giphy.gif",
            "mxc://maplehouse.tinhat.me/abc",
            "/api/games/art/valheim/portrait",
            "https://element.example.com/",
            "https://element.example.com/#/room/#general:example.com",
            "https://element.example.com/apix/soundboard",
            "https://element.example.com.evil.example/api/x",
            "http://element.example.com/api/x",
            "https://repo.example.com/native/latest.json",
        ] {
            assert_eq!(rebase_onto(u, MINE, &k), u, "{u}");
        }
    }

    #[test]
    fn the_known_bases_are_today_s_and_the_former_names_the_server_lists() {
        assert_eq!(known_from("", &["alpha.local".into()]), Vec::<String>::new(), "signed out: nothing is ours");
        assert_eq!(known_from("https://chat.crew.test/api", &[]), ["https://chat.crew.test/api"]);
        assert_eq!(
            known_from("https://chat.crew.test/api", &["alpha.local".into(), "home.local".into(), "alpha.local".into()]),
            ["https://chat.crew.test/api", "https://alpha.local/api", "https://home.local/api"]
        );
        // a server this device used before (another account) is not on the list: its links are left alone
        let k = known_from("https://chat.crew.test/api", &["alpha.local".into()]);
        assert_eq!(rebase_onto("https://other.example/api/soundboard/clip?id=1&sig=x", "https://chat.crew.test/api", &k), "https://other.example/api/soundboard/clip?id=1&sig=x");
        assert_eq!(rebase_onto("https://alpha.local/api/vods/file/r/1/a.mka?sig=y", "https://chat.crew.test/api", &k), "https://chat.crew.test/api/vods/file/r/1/a.mka?sig=y");
    }

    #[test]
    fn user_agent_names_the_build_and_the_os() {
        assert_eq!(agent_for("10.5.2", "linux"), "TinHat/10.5.2 (linux)");
        assert_eq!(agent_for("10.5.2", "windows"), "TinHat/10.5.2 (windows)");
        let ua = user_agent();
        assert_eq!(ua, format!("TinHat/{} ({})", env!("CARGO_PKG_VERSION"), std::env::consts::OS));
        // what a server's log classifier keys on: `Tin ?Hat[/ ](\d[\w.\-+]*)` + an optional `(os)`
        let v = ua.strip_prefix("TinHat/").expect("TinHat/ prefix");
        assert!(v.starts_with(|c: char| c.is_ascii_digit()), "{ua}");
        assert!(ua.is_ascii() && !ua.contains(['\r', '\n']), "a valid header value: {ua}");
    }

    /// The agent really goes out on the wire (a one-shot local HTTP server reads the request).
    #[test]
    fn api_requests_carry_the_user_agent() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            sock.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
            let mut req = Vec::new();
            let mut buf = [0u8; 4096];
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = sock.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                req.extend_from_slice(&buf[..n]);
            }
            sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
            String::from_utf8_lossy(&req).to_string()
        });
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let body = rt.block_on(async {
            let resp = http(Some(std::time::Duration::from_secs(10))).get(format!("http://127.0.0.1:{port}/api/gifs/trending")).send().await.unwrap();
            resp.text().await.unwrap()
        });
        assert_eq!(body, "{}");
        let req = server.join().unwrap();
        let ua = req.lines().find_map(|l| l.strip_prefix("user-agent: ").or_else(|| l.strip_prefix("User-Agent: "))).unwrap_or("");
        assert_eq!(ua, user_agent(), "request was:\n{req}");
    }

    #[test]
    fn memories_json_is_rebased_everywhere_it_carries_a_link() {
        let k = known(&[MINE, ELEMENT]);
        let mut v = serde_json::json!({
            "sessions": [{
                "id": "1789",
                "mixed": "https://element.example.com/api/vods/file/r/1789/mixed.mka?exp=1&sig=a",
                "tracks": [
                    { "file": "a.mka", "url": "https://element.example.com/api/vods/file/r/1789/a.mka?exp=1&sig=b",
                      "combined_url": "https://element.example.com/api/vods/file/r/1789/combined-a.mkv?exp=1&sig=c",
                      "kind": "audio", "user": "@a:example.com" },
                ],
                "live": false, "count": 3
            }]
        });
        rebase_json_onto(&mut v, MINE, &k);
        let s = &v["sessions"][0];
        assert_eq!(s["mixed"], "https://maplehouse.tinhat.me/api/vods/file/r/1789/mixed.mka?exp=1&sig=a");
        assert_eq!(s["tracks"][0]["url"], "https://maplehouse.tinhat.me/api/vods/file/r/1789/a.mka?exp=1&sig=b");
        assert_eq!(s["tracks"][0]["combined_url"], "https://maplehouse.tinhat.me/api/vods/file/r/1789/combined-a.mkv?exp=1&sig=c");
        assert_eq!(s["tracks"][0]["user"], "@a:example.com");
        assert_eq!(s["count"], 3);
        assert_eq!(s["live"], false);
    }

    /// Signed out (no override, no server): there is no base, and a call says so instead of asking a built-in host.
    #[test]
    fn without_a_server_there_is_no_base() {
        // (another test of this binary may have laid an offer over the process-wide store: then a base exists)
        let b = base();
        if b.is_empty() {
            assert_eq!(url("/gifs/trending").unwrap_err(), NO_SERVER);
            assert_eq!(rebase_url("https://x.test/api/y"), "https://x.test/api/y");
        } else {
            assert_eq!(url("/gifs/trending").unwrap(), format!("{b}/gifs/trending"));
        }
    }
}
