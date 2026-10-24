//! StreamController — drives the E2EE NVENC screen-share publish from QML.
//!
//! Flow on `startShare(roomId)`:
//!   1. resolve the room's active MatrixRTC call focus (the real LiveKit room),
//!   2. mint a LiveKit publish token via lk-jwt-service (`/sfu/get`),
//!   3. announce our own call.member so peers subscribe to us,
//!   4. generate a 16-byte media key + BROADCAST it over Matrix
//!      (`io.element.call.encryption_keys`) so peers can decrypt our stream,
//!   5. pop the KDE screen-cast portal (user picks a monitor/window),
//!   6. publish via the LiveKit SDK with E2EE (libwebrtc NVENC encode + GCM
//!      FrameCryptor keyed by HKDF(our key) — byte-matches Element Call).
//!
//! Async work runs on the shared tokio runtime; results post back to the Qt
//! thread via `qt_thread().queue()`.

use core::pin::Pin;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use tinhat_streamer as streamer;

#[cxx_qt::bridge]
pub mod qobject {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name] // snake_case Rust -> camelCase QML (startShare, stopShare, roomId)
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(bool, sharing)] //    QML: sharing — publish is live
        #[qproperty(bool, preparing)] //  QML: preparing — start sequence in flight
        #[qproperty(QString, status)] //  QML: status
        #[qproperty(QString, room_id)] // QML: roomId — room currently shared to
        type StreamController = super::StreamControllerRust;

        /// QML: streamController.startShare(roomId)
        #[qinvokable]
        fn start_share(self: Pin<&mut StreamController>, room_id: &QString);
        /// QML: streamController.stopShare()
        #[qinvokable]
        fn stop_share(self: Pin<&mut StreamController>);
    }

    impl cxx_qt::Threading for StreamController {}
}

#[derive(Default)]
pub struct StreamControllerRust {
    sharing: bool,
    preparing: bool,
    status: QString,
    room_id: QString,
    // The running E2EE publish (LiveKit room + capture pipeline + portal cast).
    handle: Option<streamer::E2eePublish>,
    // The in-flight start sequence — aborted by stopShare() to cancel "Preparing".
    start_task: Option<tokio::task::JoinHandle<()>>,
}

impl qobject::StreamController {
    pub fn start_share(mut self: Pin<&mut Self>, room_id: &QString) {
        // Re-entrancy guard: ignore taps while a start is in flight or already live.
        if *self.sharing() || *self.preparing() {
            return;
        }
        let room_id = room_id.to_string();
        self.as_mut().set_room_id(QString::from(&room_id));
        self.as_mut().set_preparing(true);
        self.as_mut()
            .set_status(QString::from("Preparing screen share…"));

        let qt = self.qt_thread();
        let task = crate::bridge::runtime().spawn(async move {
            let result = do_start_share(&room_id).await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().rust_mut().start_task = None; // start finished
                o.as_mut().set_preparing(false);
                match result {
                    Ok((handle, label)) => {
                        eprintln!("[stream] {label}");
                        o.as_mut().rust_mut().handle = Some(handle);
                        o.as_mut().set_sharing(true);
                        o.as_mut().set_status(QString::from(&label));
                    }
                    Err(e) => {
                        eprintln!("[stream] start failed: {e}");
                        o.as_mut().set_sharing(false);
                        o.as_mut()
                            .set_status(QString::from(&format!("Screen share failed: {e}")));
                    }
                }
            });
        });
        self.as_mut().rust_mut().start_task = Some(task);
    }

    /// Stop sharing AND withdraw our call membership — works mid-"Preparing" too
    /// (aborts the in-flight start). Effectively "leave the call" from our side.
    pub fn stop_share(mut self: Pin<&mut Self>) {
        // Cancel an in-flight start (unsticks "Preparing").
        if let Some(t) = self.as_mut().rust_mut().start_task.take() {
            t.abort();
        }
        let room_id = self.room_id().to_string();
        if let Some(handle) = self.as_mut().rust_mut().handle.take() {
            crate::bridge::runtime().spawn(async move { handle.close().await });
        }
        self.as_mut().set_sharing(false);
        self.as_mut().set_preparing(false);
        self.as_mut().set_status(QString::from("Stopped"));
        spawn_leave_rtc(room_id);
    }
}

/// Withdraw our MatrixRTC membership for `room_id` (best-effort).
fn spawn_leave_rtc(room_id: String) {
    if room_id.is_empty() {
        return;
    }
    crate::bridge::runtime().spawn(async move {
        if let Some(client) = crate::session::client() {
            if let Ok(rid) = matrix_sdk::ruma::RoomId::parse(&room_id) {
                if let Some(room) = client.get_room(&rid) {
                    let _ = crate::rtc::leave_rtc_session(&room).await;
                }
            }
        }
    });
}

/// The async start sequence. Returns the live handle + a status label, or an error.
async fn do_start_share(room_id: &str) -> Result<(streamer::E2eePublish, String), String> {
    let client = crate::session::client().ok_or_else(|| "Not signed in".to_string())?;
    let rid = matrix_sdk::ruma::RoomId::parse(room_id).map_err(|e| format!("bad room id: {e}"))?;
    let room = client
        .get_room(&rid)
        .ok_or_else(|| "Room not found".to_string())?;

    // (No sync here — the single background sync loop keeps call.member fresh.
    //  A concurrent sync_once would deadlock the crypto-store lock.)

    // 1. Resolve the active call's real LiveKit focus (broker url + room alias).
    let focus = crate::rtc::resolve_focus(&room)
        .await
        .map_err(|e| format!("call lookup failed: {e}"))?
        .ok_or_else(|| {
            "No active call in this room — start the call (web/phone) first, then share.".to_string()
        })?;
    eprintln!("[stream] focus resolved: alias={} url={}", focus.alias, focus.service_url);

    // 2. Mint a LiveKit publish token for that exact room (matches web/phone peers).
    let sfu = crate::livekit::request_sfu_config(&client, &focus.service_url, &focus.alias).await?;
    eprintln!("[stream] token minted: sfu={} jwt_len={}", sfu.url, sfu.jwt.len());

    // 3. Announce our membership so peers subscribe to our track (best-effort).
    if let Err(e) = crate::rtc::join_rtc_session(&room, &focus.service_url).await {
        eprintln!("[stream] join_rtc_session warning: {e}");
    }

    // 4. Generate our media key + broadcast it over Matrix so peers can decode us.
    //    Our LiveKit identity is userId:deviceId (what lk-jwt put in the JWT, and
    //    what EC keys the media key by).
    let device_id = client.device_id().map(|d| d.to_string()).unwrap_or_default();
    let user_id = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let identity = format!("{user_id}:{device_id}");
    // Advancing index for the same reason as do_join: a fresh session must never
    // collide with a previous key in peers' (identity,index) tracking.
    let key_index = crate::e2ee_keys::advance_persisted_key_index();
    let (key, _) = crate::e2ee_keys::make_sender_key();
    // Encrypted to-device to every current call member — the same transport the call
    // path uses. The room-event broadcast this used to do was retired on 2026-09-21
    // (plaintext in unencrypted rooms; notification spam in encrypted ones).
    crate::call_ctl::announce_key_to_members(&client, &room, &user_id, &device_id, key, key_index).await;
    eprintln!("[stream] media key announced over to-device (identity={identity}, index={key_index})");

    // 5. KDE portal screen pick (on-screen; needs the user present).
    eprintln!("[stream] opening screen-cast portal…");
    let screencast = streamer::pick_screencast()
        .await
        .map_err(|e| format!("screen pick: {e}"))?;
    eprintln!("[stream] portal picked: fd={} node={}", screencast.raw_fd(), screencast.node_id());

    // 6. Publish via the LiveKit SDK with E2EE (NVENC encode + GCM FrameCryptor).
    let handle =
        streamer::start_e2ee_screenshare(&sfu.url, &sfu.jwt, &identity, key, key_index, screencast)
            .await
            .map_err(|e| format!("publish: {e}"))?;
    eprintln!("[stream] E2EE publish live");

    Ok((handle, format!("Sharing to {}", focus.alias)))
}
