//! play_net.rs — Tin Hat mini games v1: the network half of the play client (GAMES-V1-SPEC.md §6).
//!
//!   • sign-in (§6.1): a Matrix OpenID token from my own homeserver → `POST <api>/play/v1/session` → a play session
//!     (32 random bytes, 8 h). The session is a bearer for the play API only (`Authorization: Play <session>`), kept in
//!     memory, never written to disk or a log.
//!   • HTTP (§6.2): `GET /me`, `POST|DELETE /wallet`, `GET /rooms/{room}/tables`, `POST /rooms/{room}/tables`,
//!     `GET /tables/{id}/summary`.
//!   • the WebSocket (§6.3): one per app, on the tokio-tungstenite already in the tree (LiveKit's). The session goes in
//!     the first frame (`auth`), never the URL. Pings every 20 s; a dropped socket reconnects with a backoff (1 s → 30 s)
//!     and re-sends the wanted `sub` + `open`, so the server's next `view` is a full snapshot again. `bye` `session` =
//!     sign in again; `bye` `off` = games were switched off (stop); `bye` `replaced` = stop (another of my apps took
//!     the slot).
//!
//! Everything here is plain async Rust; play_ctl.rs carries the results to QML. Frames we receive pass
//! `play_wire::frame_ok` (size, depth, integer range) before anyone reads them.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::play_wire;

/// Where a server's play API lives: `<api>/play/v1` (the `api` its well-known announces, or `<homeserver>/api`).
pub fn play_base(api_base: &str) -> String {
    format!("{}/play/v1", api_base.trim_end_matches('/'))
}

/// The WebSocket URL for a play base: https → wss, http → ws (a local lab server).
pub fn ws_url(play_base: &str) -> Option<String> {
    if let Some(rest) = play_base.strip_prefix("https://") {
        Some(format!("wss://{rest}/ws"))
    } else {
        play_base.strip_prefix("http://").map(|rest| format!("ws://{rest}/ws"))
    }
}

/// A play session for one server (the `POST /session` answer).
#[derive(Clone, Debug, Default)]
pub struct Session {
    pub token: String,
    pub user: String,
    pub expires_ms: u64,
    /// rev 2: the spaces on this server where I have a wallet
    pub wallets: Vec<String>,
    pub rig_hidden: bool,
}

impl Session {
    pub fn valid(&self, now_ms: u64) -> bool {
        !self.token.is_empty() && now_ms + 60_000 < self.expires_ms
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// An HTTP answer's error, in the contract's words: `{status, code}` (`code` from the body's `{"code": …}`).
#[derive(Clone, Debug)]
pub struct ApiError {
    pub status: u16,
    pub code: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.status, self.code)
    }
}

fn net_err(e: impl std::fmt::Display) -> ApiError {
    // never echo a URL with a token in it: our URLs carry none (the session goes in a header)
    eprintln!("[play] network: {e}");
    ApiError { status: 0, code: "network".into() }
}

async fn answer(resp: matrix_sdk::reqwest::Response) -> Result<Value, ApiError> {
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(net_err)?;
    let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        Ok(body)
    } else {
        let code = body.get("code").and_then(Value::as_str).unwrap_or("").to_owned();
        Err(ApiError { status, code: if code.is_empty() { format!("http-{status}") } else { code } })
    }
}

/// The play API's HTTP client: the Tin Hat User-Agent, 15 s, and NO redirects (a session or an OpenID token never
/// follows a redirect to another host).
fn http() -> Result<matrix_sdk::reqwest::Client, ApiError> {
    use matrix_sdk::reqwest;
    static C: std::sync::OnceLock<Option<reqwest::Client>> = std::sync::OnceLock::new();
    // never a fallback client: a default one would follow redirects (and a 307/308 would carry the OpenID body away)
    C.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(crate::proxy::user_agent())
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .build()
            .ok()
    })
    .clone()
    .ok_or(ApiError { status: 0, code: "network".into() })
}

/// `client.kind` for this build (§6.1): always "desktop" (the Mac app is its own shell).
pub const CLIENT_KIND: &str = "desktop";

/// §6.1: OpenID token from my homeserver → a play session at `base`.
pub async fn sign_in(client: &matrix_sdk::Client, base: &str) -> Result<Session, ApiError> {
    let user = client.user_id().map(|u| u.to_string()).ok_or(ApiError { status: 0, code: "signed-out".into() })?;
    note_request("POST", "/session");
    let openid = client.account().request_openid_token().await.map_err(net_err)?;
    let body = json!({
        "openid": {
            "access_token": openid.access_token,
            "matrix_server_name": openid.matrix_server_name.to_string(),
            "token_type": openid.token_type.to_string(),
            "expires_in": openid.expires_in.as_secs(),
        },
        "user_id": user,
        "client": {"kind": CLIENT_KIND, "v": env!("CARGO_PKG_VERSION"), "pad": false},
    });
    let resp = http()?
        .post(format!("{base}/session"))
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(net_err)?;
    let v = answer(resp).await?;
    let token = v.get("session").and_then(Value::as_str).unwrap_or("").to_owned();
    if token.is_empty() || token.len() > 128 {
        return Err(ApiError { status: 200, code: "bad-session".into() });
    }
    Ok(Session {
        token,
        user: v.get("user").and_then(Value::as_str).unwrap_or(&user).to_owned(),
        expires_ms: v.get("expires").and_then(Value::as_u64).unwrap_or_else(|| now_ms() + 8 * 3600 * 1000),
        wallets: v
            .get("wallets")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).filter(|s| s.starts_with('!')).map(str::to_string).collect())
            .unwrap_or_default(),
        rig_hidden: v.get("rig_hidden").and_then(Value::as_bool).unwrap_or(false),
    })
}

/// One HTTP call with the play session: `method` GET / POST / DELETE, `path` from `/` (already percent-encoded).
pub async fn call(base: &str, session: &str, method: &str, path: &str, body: Option<&Value>) -> Result<Value, ApiError> {
    let http = http()?;
    note_request(method, path);
    let url = format!("{base}{path}");
    let req = match method {
        "POST" => http.post(&url),
        "DELETE" => http.delete(&url),
        _ => http.get(&url),
    };
    let mut req = req.header("Authorization", format!("Play {session}"));
    if let Some(b) = body {
        req = req.header("Content-Type", "application/json").body(b.to_string());
    }
    let a = answer(req.send().await.map_err(net_err)?).await;
    if let Ok(v) = &a {
        record("http", &format!("{method} {path}"), &v.to_string());
    }
    a
}

/// Gate hook (`cfg(tinhat_env_selftests)`, listed in scripts/release-binary-check.sh): `TINHAT_PLAY_RECORD=<file>`
/// appends every play API answer and every server frame, as one JSON line each, so the tests can run on shapes recorded
/// from a real lab server (real-shaped fixtures). Never the session exchange, never `hello`/`auth`.
#[cfg(tinhat_env_selftests)]
pub fn record(kind: &str, what: &str, body: &str) {
    let Ok(p) = std::env::var("TINHAT_PLAY_RECORD") else { return };
    if what.contains("/session") || what == "hello" || what == "auth" {
        return;
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let _ = writeln!(f, "{}", json!({"kind": kind, "what": what, "body": body}));
    }
}
#[cfg(not(tinhat_env_selftests))]
#[inline]
pub fn record(_: &str, _: &str, _: &str) {}

/// Gate hook (`cfg(tinhat_env_selftests)`, in release-binary-check.sh): `TINHAT_PLAY_OFF=1` answers every well-known as
/// a server WITHOUT games would (`features.play` dropped), at both discovery sites (play_ctl + prestige): the play-OFF row
/// (G6) proves nothing of the games shows and not one play request goes out.
#[cfg(tinhat_env_selftests)]
pub fn well_known_seen(v: &mut Value) {
    if std::env::var_os("TINHAT_PLAY_OFF").is_some() {
        if let Some(f) = v.pointer_mut("/dev.tinhat.server/features").and_then(Value::as_object_mut) {
            f.remove("play");
        }
    }
}
#[cfg(not(tinhat_env_selftests))]
#[inline]
pub fn well_known_seen(_: &mut Value) {}

/// One line per play request (the path up to its query: no session, no body), so a play-OFF run can count zero.
pub fn note_request(method: &str, path: &str) {
    eprintln!("[play] http {method} {}", path.split('?').next().unwrap_or(""));
}

/// A room id in a URL path (`!abc:x` → `%21abc%3Ax`).
pub fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>().replace('+', "%20")
}

// ---------------------------------------------------------------------------------------------------------------
// The WebSocket
// ---------------------------------------------------------------------------------------------------------------

/// What the socket tells its owner.
#[derive(Debug)]
pub enum SockEvent {
    /// "connecting" | "online" | "retrying" | "stopped:<code>"
    Status(String),
    /// A received frame that passed `frame_ok`.
    Frame(Value),
    /// The session was refused (`bye session` or an auth failure): the owner signs in again and restarts the socket.
    SessionExpired,
}

/// What the socket should be subscribed to after every (re)connect.
#[derive(Default, Debug, Clone)]
pub struct Wanted {
    pub room: Option<String>,
    pub table: Option<String>,
}

/// A running socket: send frames with `send`; drop or `stop` it to close.
pub struct Sock {
    tx: mpsc::UnboundedSender<Value>,
    pub wanted: Arc<Mutex<Wanted>>,
    stop: Arc<tokio::sync::Notify>,
}

impl Sock {
    pub fn send(&self, frame: Value) -> bool {
        self.tx.send(frame).is_ok()
    }
    pub fn stop(&self) {
        self.stop.notify_waiters();
        self.stop.notify_one();
    }
}

impl Drop for Sock {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start the socket for `play_base` with `session`. `on` gets every event (from a tokio thread).
pub fn start(
    rt: &tokio::runtime::Handle,
    play_base: &str,
    session: String,
    wanted: Wanted,
    on: Arc<dyn Fn(SockEvent) + Send + Sync>,
) -> Option<Sock> {
    let url = ws_url(play_base)?;
    let (tx, rx) = mpsc::unbounded_channel::<Value>();
    let wanted = Arc::new(Mutex::new(wanted));
    let stop = Arc::new(tokio::sync::Notify::new());
    rt.spawn(run(url, session, wanted.clone(), rx, stop.clone(), on));
    Some(Sock { tx, wanted, stop })
}

enum End {
    Stopped,
    Retry,
    Bye(String),
}

async fn run(
    url: String,
    session: String,
    wanted: Arc<Mutex<Wanted>>,
    mut rx: mpsc::UnboundedReceiver<Value>,
    stop: Arc<tokio::sync::Notify>,
    on: Arc<dyn Fn(SockEvent) + Send + Sync>,
) {
    let mut backoff = Duration::from_secs(1);
    loop {
        on(SockEvent::Status("connecting".into()));
        let end = tokio::select! {
            _ = stop.notified() => End::Stopped,
            e = once(&url, &session, &wanted, &mut rx, &on) => e,
        };
        match end {
            End::Stopped => {
                on(SockEvent::Status("stopped:closed".into()));
                return;
            }
            End::Bye(code) if code == "session" => {
                on(SockEvent::SessionExpired);
                return;
            }
            End::Bye(code) => {
                on(SockEvent::Status(format!("stopped:{code}")));
                return;
            }
            End::Retry => {
                eprintln!("[play] socket: retrying in {} s", backoff.as_secs());
                on(SockEvent::Status("retrying".into()));
                tokio::select! {
                    _ = stop.notified() => { on(SockEvent::Status("stopped:closed".into())); return; }
                    _ = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }
}

async fn once(
    url: &str,
    session: &str,
    wanted: &Arc<Mutex<Wanted>>,
    rx: &mut mpsc::UnboundedReceiver<Value>,
    on: &Arc<dyn Fn(SockEvent) + Send + Sync>,
) -> End {
    eprintln!("[play] socket: connecting");
    let conn = tokio::time::timeout(Duration::from_secs(15), tokio_tungstenite::connect_async(url)).await;
    let Ok(Ok((ws, _))) = conn else {
        eprintln!("[play] socket: could not connect");
        return End::Retry;
    };
    let (mut sink, mut stream) = ws.split();
    // anything clicked while the socket was down is dropped, never replayed onto a later state (moves name their turn,
    // but table-level moves like deal/start don't)
    while rx.try_recv().is_ok() {}
    use tokio_tungstenite::tungstenite::Message;
    let auth = json!({"t": "auth", "session": session, "client": {"kind": CLIENT_KIND, "pad": false}});
    if sink.send(Message::text(auth.to_string())).await.is_err() {
        return End::Retry;
    }
    let mut greeted = false;
    let mut ping = tokio::time::interval(Duration::from_secs(20));
    ping.tick().await;
    let mut last_heard = std::time::Instant::now();
    loop {
        tokio::select! {
            msg = stream.next() => {
                let Some(Ok(msg)) = msg else { return End::Retry };
                last_heard = std::time::Instant::now();
                let text = match msg {
                    Message::Text(t) => t.to_string(),
                    Message::Close(_) => return End::Retry,
                    _ => continue,
                };
                if cfg!(tinhat_env_selftests) {
                    let t = serde_json::from_str::<Value>(&text).ok().and_then(|v| v.get("t").and_then(Value::as_str).map(str::to_owned));
                    record("frame", t.as_deref().unwrap_or("?"), &text);
                }
                let Some(frame) = play_wire::frame_ok(&text).filter(play_wire::server_frame_ok) else {
                    eprintln!("[play] socket: dropped a malformed frame ({} bytes)", text.len());
                    continue;
                };
                match frame.get("t").and_then(Value::as_str) {
                    Some("hello") if !greeted => {
                        greeted = true;
                        eprintln!("[play] socket: online");
                        on(SockEvent::Status("online".into()));
                        let w = wanted.lock().map(|w| w.clone()).unwrap_or_default();
                        if let Some(room) = w.room {
                            if !send_timed(&mut sink, json!({"t": "sub", "room": room})).await {
                                return End::Retry;
                            }
                        }
                        if let Some(table) = w.table {
                            if !send_timed(&mut sink, json!({"t": "open", "table": table})).await {
                                return End::Retry;
                            }
                        }
                    }
                    Some("bye") => {
                        let code = frame.get("code").and_then(Value::as_str).unwrap_or("closed").to_owned();
                        return End::Bye(code);
                    }
                    // a refused session answered as an error frame: sign in again (never retry with the same session)
                    Some("error") if frame.get("code").and_then(Value::as_str) == Some("session") => {
                        return End::Bye("session".into());
                    }
                    _ => {}
                }
                on(SockEvent::Frame(frame));
            }
            // nothing goes out before the server's hello (the session is checked first)
            out = rx.recv(), if greeted => {
                let Some(out) = out else { return End::Stopped };
                if !send_timed(&mut sink, out).await {
                    return End::Retry;
                }
            }
            _ = ping.tick() => {
                if last_heard.elapsed() > Duration::from_secs(60) {
                    eprintln!("[play] socket: silent for 60 s, reconnecting");
                    return End::Retry;
                }
                if !send_timed(&mut sink, json!({"t": "ping"})).await {
                    return End::Retry;
                }
                eprintln!("[play] socket: ping (heard {} s ago)", last_heard.elapsed().as_secs());
            }
        }
    }
}

/// One frame out, given at most 10 s (a stalled write must not stop the reads and the silence check for long).
async fn send_timed<S>(sink: &mut S, frame: Value) -> bool
where
    S: futures_util::Sink<tokio_tungstenite::tungstenite::Message> + Unpin,
{
    use tokio_tungstenite::tungstenite::Message;
    matches!(tokio::time::timeout(Duration::from_secs(10), sink.send(Message::text(frame.to_string()))).await, Ok(Ok(())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(play_base("https://lab.test/api/"), "https://lab.test/api/play/v1");
        assert_eq!(ws_url("https://lab.test/api/play/v1").unwrap(), "wss://lab.test/api/play/v1/ws");
        assert_eq!(ws_url("http://127.0.0.1:3001/api/play/v1").unwrap(), "ws://127.0.0.1:3001/api/play/v1/ws");
        assert!(ws_url("ftp://x").is_none());
        assert_eq!(enc("!abc:lab.test"), "%21abc%3Alab.test");
    }

    #[test]
    fn session_validity() {
        let s = Session { token: "x".into(), expires_ms: 1_000_000, ..Default::default() };
        assert!(s.valid(900_000));
        assert!(!s.valid(950_000), "renewed a minute early");
        assert!(!Session::default().valid(0));
    }
}
