//! Process-global handle to the logged-in matrix-sdk `Client`.
//!
//! CXX-Qt QObjects are `!Send`, so controllers can't hold references to each
//! other. Instead the `Client` (which is `Clone + Send`, internally `Arc`) lives
//! here: `LoginController` publishes it on login/restore success, and any other
//! controller (`StreamController`, `RoomController`) clones it out for its async
//! tokio tasks. `logout()` clears it.

use matrix_sdk::Client;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use tokio::task::JoinHandle;

static CLIENT: OnceLock<Arc<RwLock<Option<Client>>>> = OnceLock::new();
static SYNC: OnceLock<Mutex<Option<JoinHandle<()>>>> = OnceLock::new();
/// 10.6: bumps every time a session ends. Work that started for one session (a notification
/// being built, a call edge, a delayed answer) checks it before acting, so nothing of an old
/// session can ever act for the next one.
static GENERATION: AtomicU64 = AtomicU64::new(1);

/// The current session generation (see [`GENERATION`]).
pub fn generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}

fn cell() -> &'static Arc<RwLock<Option<Client>>> {
    CLIENT.get_or_init(|| Arc::new(RwLock::new(None)))
}

fn sync_cell() -> &'static Mutex<Option<JoinHandle<()>>> {
    SYNC.get_or_init(|| Mutex::new(None))
}

/// Publish the logged-in client (call from a successful login/restore).
pub fn set_client(client: Client) {
    *cell().write().unwrap() = Some(client);
}

/// Record the sole background-sync task, aborting any previous one (idempotent
/// across re-logins — a second login can't leak a second sync, which would
/// deadlock on the crypto-store lock).
pub fn set_sync_handle(handle: JoinHandle<()>) {
    let mut g = sync_cell().lock().unwrap();
    if let Some(old) = g.take() {
        old.abort();
    }
    *g = Some(handle);
}

/// Drop the shared client AND stop its background sync (call from logout).
pub fn clear_client() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    if let Some(h) = sync_cell().lock().unwrap().take() {
        h.abort();
    }
    *cell().write().unwrap() = None;
}

/// Cheap clone of the current client for an async task, if signed in.
pub fn client() -> Option<Client> {
    cell().read().unwrap().clone()
}

/// The signed-in account's server name (`@user:server` → `server`), if signed in.
pub fn server_name() -> Option<String> {
    client()?.user_id().map(|u| u.server_name().to_string())
}

/// The session's homeserver origin (`https://host[:port]`, no path), if signed in — where a Tin Hat Server serves
/// its API (`/api`), its guest call page (`/call/`) and its call service (`/livekit/jwt`). The app has no server
/// of its own (selfhost, 2026-10-21).
pub fn server_origin() -> Option<String> {
    client().map(|c| origin_of(c.homeserver().as_str()))
}

/// A URL's origin: `https://Host:8448/anything?x` → `https://host:8448`; a bare host gets `https://`; "" for nothing.
pub fn origin_of(url: &str) -> String {
    let u = url.trim();
    let (scheme, rest) = match u.split_once("://") {
        Some((s, r)) => (s.to_ascii_lowercase(), r),
        None => ("https".to_string(), u),
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("").trim();
    if host.is_empty() {
        return String::new();
    }
    format!("{scheme}://{}", host.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::origin_of;

    #[test]
    fn the_origin_of_a_homeserver_url() {
        assert_eq!(origin_of("https://maplehouse.tinhat.me/"), "https://maplehouse.tinhat.me");
        assert_eq!(origin_of("https://Maplehouse.tinhat.me/_matrix/client?x=1#y"), "https://maplehouse.tinhat.me");
        assert_eq!(origin_of("http://alpha.local:8008"), "http://alpha.local:8008");
        assert_eq!(origin_of("chat.crew.test"), "https://chat.crew.test");
        assert_eq!(origin_of(""), "");
        assert_eq!(origin_of("https://"), "");
    }
}
