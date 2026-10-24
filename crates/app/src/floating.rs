//! FloatingController — registry + metadata for Feature #6 (Discord-style floating
//! chat bubbles / chat-heads).
//!
//! This controller owns ONLY the bookkeeping for the bubble system:
//!   * `open_ids_json` — the JSON array of room ids that currently have a bubble open
//!     (a Repeater/Instantiator in FloatingChats.qml renders one top-level Window per
//!     id). Capped at MAX_BUBBLES so a runaway can't exhaust windows/sync tasks.
//!   * `rooms_meta_json` — read-only per-room metadata ({id,name,isDm,avatar,unread,
//!     highlight}) for the candidate rooms, so the bubble manager can label the window
//!     title bar / chat-head badge without re-querying the SDK from QML.
//!
//! The ACTUAL timeline of each bubble comes from a per-bubble `RoomController` instance
//! created in QML (RoomController{}.open(roomId)) — NOT from this controller. That keeps
//! each bubble's sync/key task independent and, crucially, keeps this controller free of
//! any send/receipt path (receipt-safety: a bubble must NEVER mark_read()).
//!
//! `self_test` is a launch-time hook (env TINHAT_BUBBLE_TEST=<roomId>) that QML reads to
//! auto-open one offscreen bubble so the QML can be proven to instantiate without runtime
//! errors during a headless-ish smoke run.

use core::pin::Pin;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use matrix_sdk::ruma::RoomId;

/// Hard cap on concurrent bubbles (and therefore their per-bubble sync/key tasks).
const MAX_BUBBLES: usize = 6;

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
        #[qproperty(QString, open_ids_json)] //   QML: openIdsJson — JSON [roomId,…] currently-open bubbles
        #[qproperty(QString, rooms_meta_json)] // QML: roomsMetaJson — JSON [{id,name,isDm,avatar,unread,highlight}]
        #[qproperty(QString, self_test)] //       QML: selfTest — roomId to auto-open offscreen (env TINHAT_BUBBLE_TEST), else ""
        #[qproperty(i32, max_bubbles)] //         QML: maxBubbles — concurrent-bubble cap (read-only for UI)
        type FloatingController = super::FloatingControllerRust;

        /// QML: floatingController.openBubble(roomId) — register a bubble for `roomId`.
        /// Idempotent: a no-op if already open (QML focuses the existing window instead).
        /// No-op past the MAX_BUBBLES cap. Returns true if a NEW slot was taken.
        #[qinvokable]
        fn open_bubble(self: Pin<&mut FloatingController>, room_id: &QString) -> bool;
        /// QML: floatingController.closeBubble(roomId) — free a bubble's slot (the QML
        /// Window's onClosing tears down its RoomController separately).
        #[qinvokable]
        fn close_bubble(self: Pin<&mut FloatingController>, room_id: &QString);
        /// QML: floatingController.isOpen(roomId) — is there a bubble for this room?
        #[qinvokable]
        fn is_open(self: &FloatingController, room_id: &QString) -> bool;
        /// QML: floatingController.refreshMeta() — recompute roomsMetaJson off the SDK
        /// store (name/unread/avatar for the open rooms). Cheap; off the sync loop.
        #[qinvokable]
        fn refresh_meta(self: Pin<&mut FloatingController>);
    }

    impl cxx_qt::Threading for FloatingController {}
}

pub struct FloatingControllerRust {
    open_ids_json: QString,
    rooms_meta_json: QString,
    self_test: QString,
    max_bubbles: i32,
    /// Source of truth for the open-bubble registry; mirrored into `open_ids_json`.
    open_ids: Vec<String>,
}

impl Default for FloatingControllerRust {
    fn default() -> Self {
        // Surface the launch-time smoke-test room id (if any) so QML can auto-open one
        // offscreen bubble and prove the bubble Window + ConversationView load clean.
        let self_test = std::env::var("TINHAT_BUBBLE_TEST").unwrap_or_default();
        FloatingControllerRust {
            open_ids_json: QString::from("[]"),
            rooms_meta_json: QString::from("[]"),
            self_test: QString::from(&self_test),
            max_bubbles: MAX_BUBBLES as i32,
            open_ids: Vec::new(),
        }
    }
}

impl qobject::FloatingController {
    pub fn open_bubble(mut self: Pin<&mut Self>, room_id: &QString) -> bool {
        let id = room_id.to_string();
        if id.is_empty() {
            return false;
        }
        if self.rust().open_ids.iter().any(|x| x == &id) {
            return false; // already open — QML focuses the existing window
        }
        if self.rust().open_ids.len() >= MAX_BUBBLES {
            eprintln!("[floating] bubble cap ({MAX_BUBBLES}) reached; not opening {id}");
            return false;
        }
        self.as_mut().rust_mut().open_ids.push(id);
        self.as_mut().sync_open_ids();
        self.refresh_meta();
        true
    }

    pub fn close_bubble(mut self: Pin<&mut Self>, room_id: &QString) {
        let id = room_id.to_string();
        let before = self.rust().open_ids.len();
        self.as_mut().rust_mut().open_ids.retain(|x| x != &id);
        if self.rust().open_ids.len() != before {
            self.as_mut().sync_open_ids();
        }
    }

    pub fn is_open(&self, room_id: &QString) -> bool {
        let id = room_id.to_string();
        self.rust().open_ids.iter().any(|x| x == &id)
    }

    /// Re-render `open_ids_json` from the registry (the QML mirror diffs the string).
    fn sync_open_ids(mut self: Pin<&mut Self>) {
        let json = serde_json::to_string(&self.rust().open_ids).unwrap_or_else(|_| "[]".into());
        self.as_mut().set_open_ids_json(QString::from(&json));
    }

    pub fn refresh_meta(self: Pin<&mut Self>) {
        let ids = self.rust().open_ids.clone();
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let json = build_meta(&ids).await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_rooms_meta_json(QString::from(&json));
            });
        });
    }
}

/// Build read-only metadata ({id,name,isDm,avatar,unread,highlight}) for the given room
/// ids off the local SDK store (no network beyond what sync already cached). Rooms that
/// can't be resolved are skipped. Pure read — never sends/marks anything.
async fn build_meta(ids: &[String]) -> String {
    let Some(client) = crate::session::client() else {
        return "[]".into();
    };
    let mut out: Vec<serde_json::Value> = Vec::with_capacity(ids.len());
    for id in ids {
        let Ok(rid) = RoomId::parse(id) else { continue };
        let Some(room) = client.get_room(&rid) else { continue };
        let name = match room.display_name().await {
            Ok(d) => d.to_string(),
            Err(_) => room
                .name()
                .unwrap_or_else(|| room.room_id().to_string()),
        };
        let unread = crate::notify::badge_counts(&room).await; // 2026-10-23: encrypted-room mentions (notify::badge_counts)
        let is_dm = room.is_direct().await.unwrap_or(false);
        let avatar = match room.avatar_url() {
            Some(mxc) => crate::bridge::resolve_avatar(&client, &mxc).await.unwrap_or_default(),
            None => String::new(),
        };
        out.push(serde_json::json!({
            "id": room.room_id().to_string(),
            "name": name,
            "isDm": is_dm,
            "avatar": avatar,
            "unread": unread.notification_count,
            "highlight": unread.highlight_count > 0,
        }));
    }
    serde_json::to_string(&out).unwrap_or_else(|_| "[]".into())
}
