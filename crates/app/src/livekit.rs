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
    let resp = matrix_sdk::reqwest::Client::new()
        .post(&url)
        .header("Content-Type", "application/json")
        .body(serde_json::to_vec(&body).map_err(|e| e.to_string())?)
        .send()
        .await
        .map_err(|e| format!("lk-jwt-service request failed: {e}"))?;

    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("lk-jwt-service HTTP {status}: {text}"));
    }
    serde_json::from_str::<SfuConfig>(&text)
        .map_err(|e| format!("lk-jwt-service response parse failed: {e} (body: {text})"))
}
