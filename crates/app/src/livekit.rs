// (wired into the publish path next milestone; building block for now)
#![allow(dead_code)]
// LiveKit token broker: turn a logged-in Matrix session into a LiveKit JWT via the
// homeserver's lk-jwt-service (Matrix OpenID -> LiveKit JWT). This is what
// `livekitwebrtcsink` needs (url + jwt + room) to publish the NVENC track.
//
// Flow (proven against a live Synapse + lk-jwt-service):
//   client.account().request_openid_token()  -> Matrix OpenID token
//   POST {service_url}/sfu/get {room, openid_token, device_id} -> {url, jwt}
use matrix_sdk::Client;
use serde::Deserialize;

/// LiveKit connection details returned by lk-jwt-service.
#[derive(Debug, Deserialize)]
pub struct SfuConfig {
    /// LiveKit SFU websocket URL, e.g. `wss://<server>/livekit/sfu`.
    pub url: String,
    /// Signed LiveKit access token scoped to `room`.
    pub jwt: String,
}

/// Request a LiveKit token for `room` from the lk-jwt-service at `service_url`
/// (the room's RTC focus `livekit_service_url`, e.g.
/// `https://<server>/livekit/jwt`).
///
/// Calls round M1 (DESIGN §2.5): callers pass a service that `callrules::focus_decision` approved (own, trusted, or
/// the user said Join) — the OpenID token is requested here, so it leaves only after that decision, and it is POSTed
/// once, to that service only (no fallback). The answer's LiveKit address must pass `callrules::sfu_url_ok`
/// (`wss:`/`https:`, a host, no user-info); `https:` is handed to LiveKit as `wss:`.
pub async fn request_sfu_config(
    client: &Client,
    service_url: &str,
    room: &str,
) -> Result<SfuConfig, String> {
    let device_id = client.device_id().map(|d| d.to_string()).unwrap_or_default();
    request_sfu_config_with_device(client, service_url, room, &device_id).await
}

/// Like [`request_sfu_config`] but with an explicit `device_id`. lk-jwt-service maps
/// the request's `device_id` straight into the LiveKit participant identity
/// (`userId:deviceId`), so passing distinct device ids mints distinct identities for
/// the same account — used by the headless E2EE loopback self-test to play both a
/// publisher and a subscriber from one login (no second account/credentials needed).
pub async fn request_sfu_config_with_device(
    client: &Client,
    service_url: &str,
    room: &str,
    device_id: &str,
) -> Result<SfuConfig, String> {
    let openid = client
        .account()
        .request_openid_token()
        .await
        .map_err(|e| format!("OpenID token request failed: {e}"))?;

    // lk-jwt-service expects the raw Matrix OpenID token object verbatim.
    let openid_token = serde_json::json!({
        "access_token": openid.access_token,
        "token_type": openid.token_type,
        "matrix_server_name": openid.matrix_server_name,
        "expires_in": openid.expires_in.as_secs(),
    });
    let body = serde_json::json!({
        "room": room,
        "openid_token": openid_token,
        "device_id": device_id,
    });

    let url = format!("{}/sfu/get", service_url.trim_end_matches('/'));
    let text = post_sfu_get(&no_redirect_client()?, &url, serde_json::to_vec(&body).map_err(|e| e.to_string())?).await?;
    let mut cfg = serde_json::from_str::<SfuConfig>(&text)
        .map_err(|e| format!("lk-jwt-service response parse failed: {e} (body: {text})"))?;
    cfg.url = checked_sfu_url(&cfg.url)?;
    Ok(cfg)
}

/// POST the OpenID proof to `{service}/sfu/get` ONCE, to that address only: a redirect (any 3xx) is a failure, never
/// followed — a 307/308 would re-send the body, i.e. the user's sign-in proof, to wherever it points (security review
/// 12.1.0 must-fix; the mobile core's `no_redirect_client` does the same). Returns the answer's body on a 2xx.
async fn post_sfu_get(http: &matrix_sdk::reqwest::Client, url: &str, body: Vec<u8>) -> Result<String, String> {
    let resp = http
        .post(url)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| format!("lk-jwt-service request failed: {e}"))?;
    let status = resp.status();
    if status.is_redirection() {
        eprintln!("[call-host] the call service answered {status} (a redirect) — not following it, not joining");
        return Err(format!("lk-jwt-service answered with a redirect (HTTP {status}) — not followed"));
    }
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("lk-jwt-service HTTP {status}: {text}"));
    }
    Ok(text)
}

/// The HTTP client for the lk-jwt POST: reqwest's defaults (the same TLS setup as `Client::new()`, which the POST used
/// before) with redirects OFF. Built once.
fn no_redirect_client() -> Result<matrix_sdk::reqwest::Client, String> {
    use matrix_sdk::reqwest;
    static C: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    if let Some(c) = C.get() {
        return Ok(c.clone());
    }
    let c = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("HTTP client setup failed: {e}"))?;
    Ok(C.get_or_init(|| c).clone())
}

/// The call service's LiveKit address, if safe to connect to (callrules::sfu_url_ok); `https://` becomes `wss://`.
pub fn checked_sfu_url(url: &str) -> Result<String, String> {
    if !crate::callrules::sfu_url_ok(url) {
        eprintln!("[call-host] the call service answered with an unsafe LiveKit address — not connecting");
        return Err(crate::callrules::UNSAFE_SFU_TEXT.to_string());
    }
    Ok(match url.get(..8) {
        Some(p) if p.eq_ignore_ascii_case("https://") => format!("wss://{}", &url[8..]),
        _ => url.to_string(),
    })
}

#[cfg(test)]
mod tests {
    /// A one-shot HTTP server on 127.0.0.1 that answers every request with `answer` and counts the requests it got.
    fn mock(answer: &'static str) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = format!("http://{}", l.local_addr().unwrap());
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let h = hits.clone();
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(mut s) = s else { break };
                h.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let _ = s.write_all(answer.as_bytes());
            }
        });
        (addr, hits)
    }

    /// 12.1.0 must-fix: a 307 from the call service is a failure; the redirect target never sees a request (so the
    /// OpenID proof isn't re-sent anywhere); a 2xx still answers.
    #[test]
    fn the_openid_post_never_follows_a_redirect() {
        crate::proxy::test_tls_provider();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (elsewhere, elsewhere_hits) = mock("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
        let redirect: &'static str = Box::leak(
            format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: {elsewhere}/sfu/get\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_boxed_str(),
        );
        let (service, service_hits) = mock(redirect);
        let http = super::no_redirect_client().unwrap();
        let r = rt.block_on(super::post_sfu_get(&http, &format!("{service}/sfu/get"), b"{\"openid_token\":\"x\"}".to_vec()));
        let e = r.unwrap_err();
        assert!(e.contains("redirect") && e.contains("307"), "{e}");
        assert_eq!(service_hits.load(std::sync::atomic::Ordering::SeqCst), 1, "one POST, to the service");
        assert_eq!(elsewhere_hits.load(std::sync::atomic::Ordering::SeqCst), 0, "nothing re-sent to the redirect target");
        for code in ["301 Moved Permanently", "302 Found", "308 Permanent Redirect"] {
            let a: &'static str = Box::leak(format!("HTTP/1.1 {code}\r\nLocation: {elsewhere}/x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_boxed_str());
            let (svc, _) = mock(a);
            assert!(rt.block_on(super::post_sfu_get(&http, &format!("{svc}/sfu/get"), b"{}".to_vec())).is_err(), "{code}");
        }
        assert_eq!(elsewhere_hits.load(std::sync::atomic::Ordering::SeqCst), 0);
        // a plain 2xx answer still comes through
        let (ok, _) = mock("HTTP/1.1 200 OK\r\nContent-Length: 27\r\nConnection: close\r\n\r\n{\"url\":\"wss://a\",\"jwt\":\"j\"}");
        assert_eq!(rt.block_on(super::post_sfu_get(&http, &format!("{ok}/sfu/get"), b"{}".to_vec())).unwrap(), "{\"url\":\"wss://a\",\"jwt\":\"j\"}");
    }

    #[test]
    fn the_answer_is_checked_and_https_becomes_wss() {
        assert_eq!(super::checked_sfu_url("wss://a.test/livekit/sfu").as_deref(), Ok("wss://a.test/livekit/sfu"));
        assert_eq!(super::checked_sfu_url("https://a.test/livekit/sfu").as_deref(), Ok("wss://a.test/livekit/sfu"));
        assert_eq!(super::checked_sfu_url("HTTPS://a.test/x").as_deref(), Ok("wss://a.test/x"));
        for bad in ["ws://a.test/sfu", "http://a.test/sfu", "wss://u:p@a.test/sfu", "", "wss://a.test/\nx"] {
            assert_eq!(super::checked_sfu_url(bad), Err(crate::callrules::UNSAFE_SFU_TEXT.to_string()), "{bad:?}");
        }
    }
}
