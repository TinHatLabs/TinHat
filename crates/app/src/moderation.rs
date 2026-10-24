//! Call-moderation client for the Tin Hat server endpoint (docs/RBAC.md §5.4 / §6).
//!
//! Server-mute / unmute / disconnect are ENFORCED by the server's Tin Hat API
//! (`POST /api/moderation/call`): it verifies the caller's Matrix access token, reads the
//! space's roles from Synapse and acts on the SFU with its admin key. This module is the
//! HTTP client plus the `dev.tinhat.call.moderation` state helpers the client uses to
//! remember a mute across rejoins (the SFU forgets a participant's permission the moment
//! they leave; the moderator's client re-enforces from this state when they come back).

use serde_json::Value;

/// Room-state type carrying a persisted server-mute (docs/RBAC.md §3). state_key = the
/// target mxid WITHOUT its leading `@` (Matrix only lets a user send `@`-prefixed keys
/// for themselves). Written in the CALL room, not the space.
pub const CALL_MOD_TYPE: &str = "dev.tinhat.call.moderation";

/// Every HTTP round trip to the moderation endpoint is bounded (one click, one answer).
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The state_key for a member's moderation record: mxid minus the leading `@`.
pub fn state_key_for(mxid: &str) -> String {
    mxid.strip_prefix('@').unwrap_or(mxid).to_string()
}

/// 10.5.2: the API's client carries `TinHat/<version> (<os>)` ([`crate::proxy::http`]).
fn http() -> matrix_sdk::reqwest::Client {
    crate::proxy::http(Some(TIMEOUT))
}

/// POST JSON to `{proxy::base()}{path}` with a bearer token. Returns the HTTP status and
/// the parsed body (`Value::Null` when the body is empty / not JSON) — the caller decides
/// what a non-2xx means, using the server's `error` string when it sent one.
pub async fn post_json_auth(path: &str, bearer: &str, body: Value) -> Result<(u16, Value), String> {
    let url = format!("{}{}", crate::proxy::base(), path);
    let resp = http()
        .post(&url)
        .header("Authorization", format!("Bearer {bearer}"))
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                "The server didn't answer in time".to_string()
            } else {
                format!("network: {e}")
            }
        })?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    let v = serde_json::from_str::<Value>(&text).unwrap_or(Value::Null);
    Ok((status, v))
}

/// The user-facing message for a failed moderation call: the server's `error` string
/// when it sent one, else "server said <status>".
fn error_message(status: u16, body: &Value) -> String {
    match body.get("error").and_then(|e| e.as_str()).filter(|s| !s.is_empty()) {
        Some(e) => e.to_string(),
        None => format!("server said {status}"),
    }
}

/// Ask the Tin Hat server to `mute` / `unmute` / `disconnect` `target_identity`
/// (`@user:server:DEVICE`, the LiveKit identity) in the call of `room_id`. The bearer is
/// OUR Matrix access token — that is what the server's `requireMatrixUser` verifies (and
/// how it knows who is asking). Ok(body) on 2xx (`{"ok":true,"action","target","tracksMuted"}`),
/// Err(message) otherwise — the server's `error` text verbatim ("You can't moderate calls
/// in this space", "…outranks you", "They're not in the call") or "server said <status>".
pub async fn call_action(room_id: &str, action: &str, target_identity: &str) -> Result<Value, String> {
    let token = crate::session::client()
        .and_then(|c| c.access_token())
        .ok_or_else(|| "Not signed in".to_string())?;
    let body = serde_json::json!({ "room_id": room_id, "action": action, "target": target_identity });
    let (status, v) = post_json_auth("/moderation/call", &token, body).await?;
    if (200..300).contains(&status) {
        eprintln!("[moderation] {action} {target_identity} in {room_id} -> {status}");
        Ok(v)
    } else {
        let msg = error_message(status, &v);
        eprintln!("[moderation] {action} {target_identity} in {room_id} -> {status}: {msg}");
        Err(msg)
    }
}

/// The call room, from the local store.
fn call_room(room_id: &str) -> Option<matrix_sdk::Room> {
    let client = crate::session::client()?;
    let rid = matrix_sdk::ruma::RoomId::parse(room_id).ok()?;
    client.get_room(&rid)
}

/// Persist (or clear) a server-mute for `target_mxid` in the CALL room:
/// `{"muted":true,"by":"@me:server","ts":<ms>}` or `{}`.
pub async fn write_mute_state(room_id: &str, target_mxid: &str, muted: bool, by: &str) -> Result<(), String> {
    let room = call_room(room_id).ok_or_else(|| "Room not found".to_string())?;
    let content = if muted {
        serde_json::json!({ "muted": true, "by": by, "ts": crate::appstate::now_ms() })
    } else {
        serde_json::json!({})
    };
    crate::appstate::send_state(&room, CALL_MOD_TYPE, &state_key_for(target_mxid), content)
        .await
        .map_err(|e| format!("couldn't record the mute in the room: {e}"))
}

/// The moderator (mxid) who server-muted `target_mxid` per the call room's state, if the
/// record says `muted: true`. `None` = not muted / no record / cleared (`{}`).
pub async fn read_muted_by(room_id: &str, target_mxid: &str) -> Option<String> {
    let room = call_room(room_id)?;
    let v = crate::appstate::get_state(&room, CALL_MOD_TYPE, &state_key_for(target_mxid)).await?;
    muted_by_from_content(&v)
}

/// Pure half of [`read_muted_by`]: `Some(by)` when the content says `muted:true` (`by` may
/// be "" if the writer omitted it).
pub fn muted_by_from_content(content: &Value) -> Option<String> {
    if content.get("muted").and_then(|m| m.as_bool()) != Some(true) {
        return None;
    }
    Some(content.get("by").and_then(|b| b.as_str()).unwrap_or("").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_key_strips_the_at() {
        assert_eq!(state_key_for("@logan:example.com"), "logan:example.com");
        assert_eq!(state_key_for("logan:example.com"), "logan:example.com");
    }

    #[test]
    fn muted_by_reads_only_true_records() {
        let on = serde_json::json!({ "muted": true, "by": "@m:s", "ts": 1 });
        assert_eq!(muted_by_from_content(&on), Some("@m:s".to_string()));
        assert_eq!(muted_by_from_content(&serde_json::json!({ "muted": true })), Some(String::new()));
        assert_eq!(muted_by_from_content(&serde_json::json!({ "muted": false, "by": "@m:s" })), None);
        assert_eq!(muted_by_from_content(&serde_json::json!({})), None);
    }

    #[test]
    fn error_message_prefers_the_servers_text() {
        let body = serde_json::json!({ "error": "You can't moderate calls in this space" });
        assert_eq!(error_message(403, &body), "You can't moderate calls in this space");
        assert_eq!(error_message(502, &Value::Null), "server said 502");
        assert_eq!(error_message(500, &serde_json::json!({ "error": "" })), "server said 500");
    }
}
