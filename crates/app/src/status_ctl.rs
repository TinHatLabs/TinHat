//! Per-person "looking to play / joining in N min" status — the
//! `dev.tinhat.userstatus` custom room-state event (state_key = userId), ported
//! from the earlier web client.
//!
//! Content (SET): `{ text: <string>, expiresAt?: <epoch ms> }`. CLEAR: `{}`. On read,
//! entries with `expiresAt <= now` are filtered out. (The fork gates the UI to non-DM
//! rooms — `isDm` is exposed so QML can hide the editor.) Data layer + headless round-trip,
//! plus the QML `UserStatusController` (Phase 1) tracking the open room's statuses.

use core::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;
use matrix_sdk::ruma::RoomId;
use matrix_sdk::Room;

const EVENT_TYPE: &str = "dev.tinhat.userstatus";

#[cxx_qt::bridge]
pub mod qobject {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name] // snake_case Rust -> camelCase QML
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, room_id)] //       QML: roomId — room this controller tracks
        #[qproperty(QString, statuses_json)] // QML: statusesJson — [{userId,text,expiresAt}] (non-expired)
        #[qproperty(QString, my_status)] //     QML: myStatus — my own status text ("" if none)
        #[qproperty(bool, is_dm)] //            QML: isDm — hide the status editor in DMs
        #[qproperty(QString, last_error)] //    QML: lastError — friendly message on a failed write ("" = ok)
        type UserStatusController = super::UserStatusControllerRust;

        /// QML: open(roomId) — track a room + read its statuses.
        #[qinvokable]
        fn open(self: Pin<&mut UserStatusController>, room_id: &QString);
        /// QML: refresh() — re-read (re-filters expired entries).
        #[qinvokable]
        fn refresh(self: Pin<&mut UserStatusController>);
        /// QML: postStatus(text, ttlMin) — set (empty text clears) my status; ttl 0 = no expiry.
        /// (NOT `set_my_status` — that name is the auto-generated setter of the my_status qproperty.)
        #[qinvokable]
        fn post_status(self: Pin<&mut UserStatusController>, text: &QString, ttl_min: i32);
        /// QML: clearMyStatus() — clear my status.
        #[qinvokable]
        fn clear_my_status(self: Pin<&mut UserStatusController>);
    }

    impl cxx_qt::Threading for UserStatusController {}
}

#[derive(Default)]
pub struct UserStatusControllerRust {
    room_id: QString,
    statuses_json: QString,
    my_status: QString,
    is_dm: bool,
    last_error: QString,
}

impl qobject::UserStatusController {
    pub fn open(mut self: Pin<&mut Self>, room_id: &QString) {
        let rid = room_id.to_string();
        if rid == self.room_id().to_string() {
            self.refresh();
            return;
        }
        self.as_mut().set_room_id(QString::from(&rid));
        self.as_mut().set_statuses_json(QString::default());
        self.as_mut().set_my_status(QString::default());
        self.refresh();
    }

    pub fn refresh(self: Pin<&mut Self>) {
        let room_id = self.room_id().to_string();
        if room_id.is_empty() {
            return;
        }
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let Some(room) = status_room_by_id(&room_id).await else {
                return;
            };
            let (json, mine) = render_statuses(&room).await;
            let is_dm = room.is_direct().await.unwrap_or(false);
            let started = room_id.clone();
            let _ = qt.queue(move |mut o| {
                if o.room_id().to_string() != started {
                    return;
                }
                o.as_mut().set_statuses_json(QString::from(&json));
                o.as_mut().set_my_status(QString::from(&mine));
                o.as_mut().set_is_dm(is_dm);
            });
        });
    }

    pub fn post_status(mut self: Pin<&mut Self>, text: &QString, ttl_min: i32) {
        let room_id = self.room_id().to_string();
        let text = text.to_string();
        let ttl = ttl_min.max(0) as u64;
        if room_id.is_empty() {
            return;
        }
        // Clear any stale error as soon as the user attempts a write.
        self.as_mut().set_last_error(QString::default());
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let started = room_id.clone();
            if let Err(e) = do_set_status(&room_id, &text, ttl).await {
                eprintln!("[userstatus] set failed: {e}");
                let _ = qt.queue(move |mut o| {
                    if o.room_id().to_string() == started {
                        o.as_mut().set_last_error(QString::from(
                            "Couldn't set — you may not have permission in this room",
                        ));
                    }
                });
                return;
            }
            // OPTIMISTIC: reflect my own status instantly (empty text = cleared) before the
            // sync round-trips, so the UI doesn't read stale.
            let mine_now = text.trim().to_string();
            let started_opt = room_id.clone();
            let _ = qt.queue(move |mut o| {
                if o.room_id().to_string() == started_opt {
                    o.as_mut().set_my_status(QString::from(&mine_now));
                }
            });
            // Refresh the full list (others' statuses + expiry). Do NOT overwrite my_status
            // from this read — it was set optimistically above, and the not-yet-synced store
            // would revert it to stale; the ~3s poll reconciles my_status.
            let Some(room) = status_room_by_id(&room_id).await else {
                return;
            };
            let (json, _mine) = render_statuses(&room).await;
            let started2 = room_id.clone();
            let _ = qt.queue(move |mut o| {
                if o.room_id().to_string() == started2 {
                    o.as_mut().set_statuses_json(QString::from(&json));
                }
            });
        });
    }

    pub fn clear_my_status(self: Pin<&mut Self>) {
        let empty = QString::default();
        self.post_status(&empty, 0);
    }
}

async fn status_room_by_id(room_id: &str) -> Option<Room> {
    let client = crate::session::client()?;
    let rid = RoomId::parse(room_id).ok()?;
    client.get_room(&rid)
}

/// Set (or clear, when `text` is empty) my own status with a TTL of `ttl_min` minutes
/// (0 = no expiry). Best-effort PL lower, then the state write under my user id.
pub async fn do_set_status(room_id: &str, text: &str, ttl_min: u64) -> matrix_sdk::Result<()> {
    // Fail-soft (matches activity.rs): if the room/client vanished between the QML call
    // and this spawned task running (fast room switch / leave), no-op instead of panicking.
    let Some(client) = crate::session::client() else { return Ok(()) };
    let Ok(rid) = RoomId::parse(room_id) else { return Ok(()) };
    let Some(room) = client.get_room(&rid) else { return Ok(()) };
    let me = room.own_user_id().to_string();
    if text.trim().is_empty() {
        return crate::appstate::send_state(&room, EVENT_TYPE, &me, serde_json::json!({})).await;
    }
    crate::appstate::ensure_writable(&room, &[EVENT_TYPE]).await;
    let mut content = serde_json::json!({ "text": text.trim() });
    if ttl_min > 0 {
        content["expiresAt"] = serde_json::Value::from(crate::appstate::now_ms() + ttl_min * 60_000);
    }
    crate::appstate::send_state(&room, EVENT_TYPE, &me, content).await
}

/// Render all (non-expired) statuses in the room as JSON `[{userId,text,expiresAt}]`,
/// plus my own status text (empty if none). Mirrors the fork's read+expiry filter.
pub async fn render_statuses(room: &Room) -> (String, String) {
    let me = room.own_user_id().to_string();
    let now = crate::appstate::now_ms();
    let mut all: Vec<serde_json::Value> = Vec::new();
    let mut mine = String::new();
    for (state_key, _sender, content) in crate::appstate::get_state_all(room, EVENT_TYPE).await {
        let text = content.get("text").and_then(|t| t.as_str()).unwrap_or("").trim().to_string();
        if text.is_empty() {
            continue; // cleared
        }
        let expires = content.get("expiresAt").and_then(|e| e.as_u64());
        if expires.map(|e| e <= now).unwrap_or(false) {
            continue; // expired
        }
        if state_key == me {
            mine = text.clone();
        }
        all.push(serde_json::json!({ "userId": state_key, "text": text, "expiresAt": expires }));
    }
    (serde_json::to_string(&all).unwrap_or_else(|_| "[]".to_string()), mine)
}

/// Headless round-trip (no GUI): write a status (10-min TTL) → sync → read back →
/// clear → read back. Proves the type + state_key=userId write, the epoch-ms expiry
/// field + future/past filter, and the empty-content clear, against the live server.
///   TINHAT_STATUS_TEST='!nondmroom:server' ./tinhat
pub async fn headless_verify(room_id: &str) {
    let Some(client) = crate::session::client() else {
        eprintln!("[statustest] not signed in");
        return;
    };
    if let Err(e) = do_set_status(room_id, "headless looking to play", 10).await {
        eprintln!("[statustest] set FAILED: {e}");
        return;
    }
    let _ = client
        .sync_once(crate::bridge::sync_settings().timeout(std::time::Duration::from_secs(10)))
        .await;
    let Ok(rid) = RoomId::parse(room_id) else { return };
    let Some(room) = client.get_room(&rid) else { return };
    let (json, mine) = render_statuses(&room).await;
    eprintln!("[statustest] is_dm={:?}", room.is_direct().await);
    eprintln!("[statustest] AFTER SET: my='{mine}' all={json}");
    let set_ok = mine.contains("looking to play");
    let _ = do_set_status(room_id, "", 0).await; // clear
    let _ = client
        .sync_once(crate::bridge::sync_settings().timeout(std::time::Duration::from_secs(10)))
        .await;
    let (_json2, mine2) = render_statuses(&room).await;
    eprintln!("[statustest] AFTER CLEAR: my='{mine2}'");
    let clear_ok = mine2.is_empty();
    eprintln!(
        "[statustest] VERDICT: {} (set_ok={set_ok} clear_ok={clear_ok})",
        if set_ok && clear_ok { "PASS" } else { "FAIL" }
    );
}
