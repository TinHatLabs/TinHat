//! "What we're playing" per-room activity — the `dev.tinhat.activity` custom
//! room-state event (state_key `""`), ported from the earlier web client.
//!
//! Content (SET): `{ game: <non-empty trimmed string>, emoji?: <string>, setBy: <mxid>,
//! ts: <epoch ms> }`. CLEAR: `{}`. Read is valid only when `game.trim()` is non-empty.
//! Data layer + a headless write→read-back verifier, plus the QML `GameActivityController`
//! (Phase 1) that tracks the open room's activity and lets the user set/clear theirs.

use core::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;
use matrix_sdk::ruma::RoomId;
use matrix_sdk::Room;

const EVENT_TYPE: &str = "dev.tinhat.activity";

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
        #[qproperty(QString, activity_json)] // QML: activityJson — {game,emoji,setBy} or "" when none
        #[qproperty(QString, last_error)] //    QML: lastError — friendly message on a failed write ("" = ok)
        type GameActivityController = super::GameActivityControllerRust;

        /// QML: open(roomId) — track a room + read its current activity.
        #[qinvokable]
        fn open(self: Pin<&mut GameActivityController>, room_id: &QString);
        /// QML: refresh() — re-read the tracked room's activity.
        #[qinvokable]
        fn refresh(self: Pin<&mut GameActivityController>);
        /// QML: setActivity(game, emoji, code) — set our room's activity (empty game clears).
        /// `code` is the synced join/lobby code (optional): it rides on the SAME state event so
        /// the whole room sees it — fixing join codes that used to be local-only (timeline event).
        #[qinvokable]
        fn set_activity(self: Pin<&mut GameActivityController>, game: &QString, emoji: &QString, code: &QString);
        /// QML: clearActivity() — clear the room's activity.
        #[qinvokable]
        fn clear_activity(self: Pin<&mut GameActivityController>);
    }

    impl cxx_qt::Threading for GameActivityController {}
}

#[derive(Default)]
pub struct GameActivityControllerRust {
    room_id: QString,
    activity_json: QString,
    last_error: QString,
}

impl qobject::GameActivityController {
    pub fn open(mut self: Pin<&mut Self>, room_id: &QString) {
        let rid = room_id.to_string();
        if rid == self.room_id().to_string() {
            self.refresh();
            return;
        }
        self.as_mut().set_room_id(QString::from(&rid));
        self.as_mut().set_activity_json(QString::default());
        self.refresh();
    }

    pub fn refresh(self: Pin<&mut Self>) {
        let room_id = self.room_id().to_string();
        if room_id.is_empty() {
            return;
        }
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let json = load_activity_json(&room_id).await;
            let started = room_id.clone();
            let _ = qt.queue(move |mut o| {
                if o.room_id().to_string() == started {
                    o.as_mut().set_activity_json(QString::from(&json));
                }
            });
        });
    }

    pub fn set_activity(mut self: Pin<&mut Self>, game: &QString, emoji: &QString, code: &QString) {
        let room_id = self.room_id().to_string();
        let game = game.to_string();
        let emoji = emoji.to_string();
        let code = code.to_string();
        if room_id.is_empty() {
            return;
        }
        // Clear any stale error as soon as the user attempts a write.
        self.as_mut().set_last_error(QString::default());
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let Some(room) = activity_room_by_id(&room_id).await else {
                return;
            };
            let want_clear = game.trim().is_empty();
            let res = if want_clear {
                clear_activity(&room).await
            } else {
                write_activity(&room, &game, &emoji, &code).await
            };
            let started = room_id.clone();
            match res {
                Ok(()) => {
                    // OPTIMISTIC: reflect exactly what we just wrote, built from the args +
                    // our own user id — don't depend on the immediate (possibly stale) re-read.
                    let optimistic = if want_clear {
                        String::new()
                    } else {
                        serde_json::json!({
                            "game": game.trim(),
                            "emoji": emoji.trim(),
                            "code": code.trim(),
                            "setBy": room.own_user_id().to_string(),
                        })
                        .to_string()
                    };
                    let _ = qt.queue(move |mut o| {
                        if o.room_id().to_string() == started {
                            o.as_mut().set_activity_json(QString::from(&optimistic));
                        }
                    });
                    // The optimistic value already equals what a successful read returns
                    // ({game,emoji,setBy}); an immediate re-read would hit the not-yet-synced
                    // local store and revert it to stale, so leave reconciliation to the ~3s poll.
                }
                Err(e) => {
                    eprintln!("[activity] set failed: {e}");
                    let _ = qt.queue(move |mut o| {
                        if o.room_id().to_string() == started {
                            o.as_mut().set_last_error(QString::from(
                                "Couldn't set — you may not have permission in this room",
                            ));
                        }
                    });
                }
            }
        });
    }

    pub fn clear_activity(self: Pin<&mut Self>) {
        let empty = QString::default();
        self.set_activity(&empty, &empty, &empty);
    }
}

async fn activity_room_by_id(room_id: &str) -> Option<Room> {
    let client = crate::session::client()?;
    let rid = RoomId::parse(room_id).ok()?;
    client.get_room(&rid)
}

/// Current activity of `room_id` as `{game,emoji,setBy}` JSON, or "" when none/invalid.
async fn load_activity_json(room_id: &str) -> String {
    let Some(room) = activity_room_by_id(room_id).await else {
        return String::new();
    };
    match read_activity(&room).await {
        Some((game, emoji, set_by, code)) => {
            serde_json::json!({ "game": game, "emoji": emoji, "code": code, "setBy": set_by }).to_string()
        }
        None => String::new(),
    }
}

/// Read a room's current activity as `(game, emoji, set_by, code)`, or `None` if cleared.
/// `code` is the synced join/lobby code ("" when none).
pub async fn read_activity(room: &Room) -> Option<(String, String, String, String)> {
    let c = crate::appstate::get_state(room, EVENT_TYPE, "").await?;
    let game = c.get("game").and_then(|g| g.as_str()).unwrap_or("").trim().to_string();
    if game.is_empty() {
        return None; // cleared / invalid (fork: valid only when game.trim() non-empty)
    }
    let emoji = c.get("emoji").and_then(|e| e.as_str()).unwrap_or("").to_string();
    let set_by = c.get("setBy").and_then(|s| s.as_str()).unwrap_or("").to_string();
    let code = c.get("code").and_then(|s| s.as_str()).unwrap_or("").to_string();
    Some((game, emoji, set_by, code))
}

/// Set the room's activity (best-effort PL lower first, then the state write).
pub async fn write_activity(room: &Room, game: &str, emoji: &str, code: &str) -> matrix_sdk::Result<()> {
    crate::appstate::ensure_writable(room, &[EVENT_TYPE]).await;
    let mut content = serde_json::json!({
        "game": game.trim(),
        "setBy": room.own_user_id().to_string(),
        "ts": crate::appstate::now_ms(),
    });
    if !emoji.trim().is_empty() {
        content["emoji"] = serde_json::Value::from(emoji.trim());
    }
    if !code.trim().is_empty() {
        content["code"] = serde_json::Value::from(code.trim());
    }
    crate::appstate::send_state(room, EVENT_TYPE, "", content).await
}

/// Clear the room's activity (empty content).
pub async fn clear_activity(room: &Room) -> matrix_sdk::Result<()> {
    crate::appstate::send_state(room, EVENT_TYPE, "", serde_json::json!({})).await
}

/// Headless verification (no GUI): read current activity, optionally set it, sync it
/// back, read again, print a VERDICT, then clean up the sentinel. Proves the exact
/// fork schema + read-validity + PL-lower against the live homeserver.
///   TINHAT_ACTIVITY_ROOM='!room' TINHAT_ACTIVITY_SET='Helldivers 2' TINHAT_ACTIVITY_EMOJI='🎮'
pub async fn headless_verify(room_id: &str, set: Option<&str>, emoji: Option<&str>) {
    let Some(client) = crate::session::client() else {
        eprintln!("[activity] not signed in");
        return;
    };
    let Ok(rid) = RoomId::parse(room_id) else {
        eprintln!("[activity] bad room id");
        return;
    };
    let Some(room) = client.get_room(&rid) else {
        eprintln!("[activity] room not found in store");
        return;
    };
    eprintln!("[activity] before: {:?}", read_activity(&room).await);
    let Some(set) = set else {
        eprintln!("[activity] (read-only; set TINHAT_ACTIVITY_SET to test a write)");
        return;
    };
    let want_clear = set.trim().is_empty();
    let write = if want_clear {
        clear_activity(&room).await
    } else {
        write_activity(&room, set, emoji.unwrap_or(""), "").await
    };
    if let Err(e) = write {
        eprintln!("[activity] write FAILED: {e}");
        return;
    }
    // Round-trip the write into the local store.
    let _ = client
        .sync_once(crate::bridge::sync_settings().timeout(std::time::Duration::from_secs(10)))
        .await;
    let after = read_activity(&room).await;
    eprintln!("[activity] after: {after:?}");
    let pass = if want_clear {
        after.is_none()
    } else {
        after.as_ref().map(|(g, _, _, _)| g == set.trim()).unwrap_or(false)
    };
    eprintln!("[activity] VERDICT: {}", if pass { "PASS" } else { "FAIL" });
    // Leave no trace: clear the sentinel we wrote (so the test room isn't polluted).
    if !want_clear && pass {
        let _ = clear_activity(&room).await;
        eprintln!("[activity] cleaned up test value");
    }
}
