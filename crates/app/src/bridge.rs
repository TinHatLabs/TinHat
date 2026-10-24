// CXX-Qt bridge: LoginController — the Rust object QML talks to for Matrix login.
// matrix-rust-sdk runs on a tokio runtime; results are posted back to the Qt
// thread via cxx-qt's qt_thread().queue() so QML properties update safely.
//
// Session persistence: on success the MatrixSession (+ resolved homeserver URL) is
// saved to ~/.local/share/Tin Hat/session.json (0600) and the crypto/state are
// kept in a sqlite store there, so restore() on startup signs back in silently.
use core::pin::Pin;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use matrix_sdk::{authentication::matrix::MatrixSession, ruma::ServerName, Client, ClientBuilder};
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;

#[cxx_qt::bridge]
pub mod qobject {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    // auto_cxx_name: expose Rust snake_case to QML as camelCase (logged_in ->
    // loggedIn, rooms_json -> roomsJson, refresh_rooms -> refreshRooms, and the
    // NOTIFY signals too). Without it, QML's camelCase reads were `undefined`,
    // which is why the view never left the login screen.
    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, homeserver)] // QML: homeserver
        #[qproperty(QString, status)] //     QML: status
        #[qproperty(QString, user_id)] //    QML: userId
        #[qproperty(bool, busy)] //          QML: busy
        #[qproperty(bool, logged_in)] //     QML: loggedIn
        #[qproperty(bool, verified)] //      QML: verified — device cross-signed + can decrypt E2EE
        #[qproperty(bool, backup_ready)] //  QML: backupReady — 10.1: room keys are in a server-side key backup (sign-out is safe)
        #[qproperty(bool, sync_down)] //     QML: syncDown — 10.1: the sync loop has failed 3× in a row (offline / server unreachable)
        // Interactive device verification ("use another device"), the path that does NOT
        // need the Security Key. sasState drives the whole dialog:
        //   "" idle | "waiting" | "emoji" | "confirming" | "done" | "cancelled"
        #[qproperty(QString, sas_state)] //  QML: sasState
        #[qproperty(QString, sas_emoji)] //  QML: sasEmoji — JSON [{"e":"🐶","name":"Dog"}] × 7
        #[qproperty(QString, sas_peer)] //   QML: sasPeer — the other device, so the prompt can name it
        #[qproperty(QString, sas_note)] //   QML: sasNote — human-readable progress / failure reason
        #[qproperty(bool, sas_incoming)] //  QML: sasIncoming — another device asked US to verify it
        #[qproperty(bool, sas_can_start)] // QML: sasCanStart — S2: the peer lets its user pick (QR too); offer "Start emoji comparison"
        #[qproperty(QString, created_room)] // QML: createdRoom — id of the room just created/reused, so QML can open it
        #[qproperty(QString, recovery_key)] // QML: recoveryKey — shown ONCE after setting up recovery
        #[qproperty(bool, sas_can_bootstrap)] // QML: sasCanBootstrap — this dead end is fixable by setting up recovery here
        #[qproperty(QString, moved_note)] // QML: movedNote — sign-in: "OLD moved to NEW: sign in there…" (moved.rs), "" otherwise
        #[qproperty(QString, moved_to)] //   QML: movedTo — the address the note sends people to
        #[qproperty(bool, key_first)] // QML: keyFirst — Secret Storage but no identity (a moved server): the Security Key first (moved.rs)
        #[qproperty(bool, first_device_pending)] // QML: firstDevicePending — F1: the new-account check/setup runs; the verify banner + sign-in prompt wait
        #[qproperty(bool, first_device)] //         QML: firstDevice — F2: recovery-done is the first device's new Security Key
        #[qproperty(QString, server_offer_json)] // QML: serverOfferJson — selfhost: what the user's server offers {server, api, features, recorder} (server_offer.rs)
        #[qproperty(QString, rooms_json)] // QML: roomsJson — JSON [{id,name}]
        #[qproperty(QString, auto_join_room)] // QML: autoJoinRoom — self-test auto-join (env TINHAT_GUI_AUTOJOIN)
        #[qproperty(QString, auto_login)] //     QML: autoLogin — self-test sign-in "user:password" (env TINHAT_GUI_LOGIN, 10.0.6)
        #[qproperty(bool, auto_cam)] //          QML: autoCam — also enable the camera after auto-join (env TINHAT_GUI_AUTOCAM)
        #[qproperty(bool, auto_mute)] //         QML: autoMute — mute the mic ~6s after auto-join (env TINHAT_GUI_AUTOMUTE)
        #[qproperty(QString, auto_open_room)] // QML: autoOpenRoom — open a room (NO call) for screenshots (env TINHAT_GUI_OPENROOM)
        #[qproperty(QString, auto_fake_call)] // QML: autoFakeCall — render the call panel w/ placeholder tiles for screenshots (env TINHAT_GUI_FAKECALL)
        #[qproperty(QString, auto_dock)] //      QML: autoDock — env TINHAT_GUI_DOCK="corner" forces corner nav mode + pins it open for screenshots
        #[qproperty(QString, auto_theme)] //     QML: autoTheme — force a theme at launch (env TINHAT_THEME), for screenshots
        #[qproperty(QString, saved_theme)] //    QML: savedTheme — last chosen theme, restored from disk
        #[qproperty(QString, saved_nav_mode)] // QML: savedNavMode — "persistent" | "corner" nav, restored from disk
        #[qproperty(bool, saved_tour_seen)] //   QML: savedTourSeen — has the first-run onboarding tour been seen? (restored from disk)
        #[qproperty(QString, assets_path)] //    QML: assetsPath — runtime dir of bundled assets (app-icon layers, chimes)
        // Password reset by email, from the sign-in screen — i.e. for someone who is
        // by definition NOT logged in, so none of this can go through the account API.
        #[qproperty(QString, reset_stage)] //    QML: resetStage — "" idle | "sent" (link mailed) | "done"
        #[qproperty(QString, reset_status)] //   QML: resetStatus — human-readable progress / failure
        // selfhost (password.rs): how the typed server takes back a forgotten password, asked when the reset face opens
        #[qproperty(QString, reset_mode)] //     QML: resetMode — "" not asked | "checking" | "email" (the form) | "owner"
        #[qproperty(QString, reset_note)] //     QML: resetNote — the owner face's words (password::owner_reset_text)
        // 10.4: account creation with an invite code, from the sign-in screen (docs/INVITES.md).
        #[qproperty(QString, register_stage)] //  QML: registerStage — "" idle | "created" (account made, signing in)
        #[qproperty(QString, register_status)] // QML: registerStatus — human-readable progress / failure
        type LoginController = super::LoginControllerRust;

        /// 10.0: a call started in a room we are not in (callwatch.rs) — QML shows the
        /// incoming-call toast, rings, and posts a desktop notification with a Join action.
        #[qsignal]
        fn incoming_call(self: Pin<&mut LoginController>, room_id: QString, room_name: QString, who: QString);
        /// 10.0: that room's call ended (or everyone left) — toast and ring stop.
        #[qsignal]
        fn incoming_call_ended(self: Pin<&mut LoginController>, room_id: QString);
        /// 10.0: the user clicked an action on a desktop notification ("join:<roomId>").
        #[qsignal]
        fn toast_action(self: Pin<&mut LoginController>, action: QString);
        /// selfhost/firstrun §5: a DM invite was accepted (our own member event said `is_direct`) and the joined room
        /// is in rooms_json — QML opens that DM, from the room list's Accept and the DM strip alike (the web's
        /// acceptInvite does the same). Room and space invites don't emit it.
        #[qsignal]
        fn dm_accepted(self: Pin<&mut LoginController>, room_id: QString);

        // QML: login.dmTarget(text) — what the New DM field means ("blake" → "@blake:<our server>"), "" = not a user
        #[qinvokable]
        fn dm_target(self: &LoginController, text: &QString) -> QString;
        // QML: login.newRoomHint(isPrivate, isSpace, parentSpace) — New room's line: encrypted or not (new_room.rs hint,
        // the web's newRoomHint — shared vectors); "" = no line
        #[qinvokable]
        fn new_room_hint(self: &LoginController, is_private: bool, is_space: bool, parent_space: &QString) -> QString;

        // QML: loginController.login(user, password)
        #[qinvokable]
        fn login(self: Pin<&mut LoginController>, username: &QString, password: &QString);
        // QML: loginController.restore()  — try a saved session (call on startup)
        #[qinvokable]
        fn restore(self: Pin<&mut LoginController>);
        // QML: loginController.logout()
        #[qinvokable]
        fn logout(self: Pin<&mut LoginController>);
        // ---- Forgot password -------------------------------------------------
        // Same three-step flow the website uses, so a reset started in either client
        // is the same reset: ask the homeserver to mail a link, the user clicks it,
        // then set the new password proving ownership with that link's session.
        /// QML: login.requestPasswordReset(email) — mails the confirmation link.
        #[qinvokable]
        #[cxx_name = "requestPasswordReset"]
        fn request_password_reset(self: Pin<&mut LoginController>, email: &QString);
        /// QML: login.completePasswordReset(newPassword) — call AFTER the link is clicked.
        #[qinvokable]
        #[cxx_name = "completePasswordReset"]
        fn complete_password_reset(self: Pin<&mut LoginController>, new_password: &QString);
        /// QML: login.cancelPasswordReset() — back out to the sign-in form.
        #[qinvokable]
        #[cxx_name = "cancelPasswordReset"]
        fn cancel_password_reset(self: Pin<&mut LoginController>);
        /// QML: login.checkPasswordReset(server) — selfhost: the reset face opened (or its Server field changed): does
        /// the typed server mail a link, or does its owner set new passwords? (password.rs → resetMode / resetNote)
        #[qinvokable]
        #[cxx_name = "checkPasswordReset"]
        fn check_password_reset(self: Pin<&mut LoginController>, server: &QString);
        // ---- Create an account (10.4) ----------------------------------------
        /// QML: login.createAccount(username, password, inviteCode, displayName) — register with a
        /// registration token, then sign in as that account. (`register` is a C++ keyword.)
        #[qinvokable]
        #[cxx_name = "createAccount"]
        fn create_account(self: Pin<&mut LoginController>, username: &QString, password: &QString, invite_code: &QString, display_name: &QString);
        /// QML: login.cancelCreateAccount() — back to the sign-in form; clears the status.
        #[qinvokable]
        #[cxx_name = "cancelCreateAccount"]
        fn cancel_create_account(self: Pin<&mut LoginController>);
        // QML: loginController.refreshRooms() — sync + populate roomsJson
        #[qinvokable]
        fn refresh_rooms(self: Pin<&mut LoginController>);
        /// QML: login.checkMoved(server) — sign-in: has this server moved to its own domain (its well-known's
        /// dev.tinhat.moved_to)? Then movedNote says so and the Server field follows.
        #[qinvokable]
        #[cxx_name = "checkMoved"]
        fn check_moved(self: Pin<&mut LoginController>, server: &QString);
        /// QML: login.refreshServerOffer() — selfhost: ask the server what it offers again (the window came back;
        /// at most every 10 min; server_offer.rs).
        #[qinvokable]
        #[cxx_name = "refreshServerOffer"]
        fn refresh_server_offer(self: Pin<&mut LoginController>);
        /// QML: login.serverGatesJson() — the gate tables: which widget, settings row and permission a feature
        /// carries (server_offer.rs).
        #[qinvokable]
        #[cxx_name = "serverGatesJson"]
        fn server_gates_json(self: &LoginController) -> QString;
        /// QML: login.serverWebUrl() — selfhost: Tin Hat Web on the signed-in server (`<homeserver origin>/`, where a
        /// Tin Hat Server serves it); "" signed out. The invite message's "Open … in your browser".
        #[qinvokable]
        #[cxx_name = "serverWebUrl"]
        fn server_web_url(self: &LoginController) -> QString;
        // QML: loginController.verify(securityKey) — cross-sign this device + restore
        // key backup using the user's Security Key, so E2EE rooms decrypt.
        #[qinvokable]
        fn verify(self: Pin<&mut LoginController>, security_key: &QString);
        // QML: loginController.startDeviceVerification() — ask our OTHER signed-in
        // devices to verify this one by comparing emoji. No Security Key involved.
        #[qinvokable]
        fn start_device_verification(self: Pin<&mut LoginController>);
        // QML: loginController.acceptDeviceVerification() — accept a request that
        // arrived from another of our devices.
        #[qinvokable]
        fn accept_device_verification(self: Pin<&mut LoginController>);
        // QML: loginController.confirmSasMatch() — "the emoji match".
        #[qinvokable]
        fn confirm_sas_match(self: Pin<&mut LoginController>);
        // QML: loginController.cancelDeviceVerification() — cancel / dismiss.
        #[qinvokable]
        fn cancel_device_verification(self: Pin<&mut LoginController>);
        // QML: login.mismatchDeviceVerification() — "They don't match" (sends m.mismatched_sas).
        #[qinvokable]
        fn mismatch_device_verification(self: Pin<&mut LoginController>);
        // QML: login.startEmojiComparison() — S2 "Start emoji comparison" (requester, while waiting).
        #[qinvokable]
        fn start_emoji_comparison(self: Pin<&mut LoginController>);
        // QML: loginController.setUpRecovery() — create recovery + cross-signing for an
        // account that has never had it, which also verifies THIS device.
        #[qinvokable]
        fn set_up_recovery(self: Pin<&mut LoginController>, password: &QString);
        // QML: loginController.setTheme(name) — persist the chosen theme to disk.
        #[qinvokable]
        fn set_theme(self: Pin<&mut LoginController>, name: &QString);
        /// QML: login.notifyIncomingCall(roomId, roomName, who) — 10.0: the desktop notification
        /// (with its Join action) for a call the toast is showing.
        #[qinvokable]
        fn notify_incoming_call(self: Pin<&mut LoginController>, room_id: &QString, room_name: &QString, who: &QString);
        // QML: loginController.setNavMode(mode) — persist "persistent" | "corner" nav.
        #[qinvokable]
        fn set_nav_mode(self: Pin<&mut LoginController>, mode: &QString);
        // QML: loginController.setTourSeen(seen) — persist whether the first-run
        // onboarding tour has been seen (so it auto-shows exactly once).
        #[qinvokable]
        fn set_tour_seen(self: Pin<&mut LoginController>, seen: bool);
        // QML: loginController.applyColorScheme(theme) — activate the KDE color scheme
        // for `theme` (recolors the whole Kirigami app incl. chrome). GUI thread only.
        #[qinvokable]
        fn apply_color_scheme(self: Pin<&mut LoginController>, theme: &QString);
        // QML: loginController.leaveRoom(roomId) — leave a room + refresh the list.
        #[qinvokable]
        fn leave_room(self: Pin<&mut LoginController>, room_id: &QString);
        // QML: loginController.setFavourite(roomId, fav) — toggle the m.favourite tag.
        #[qinvokable]
        fn set_favourite(self: Pin<&mut LoginController>, room_id: &QString, favourite: bool);
        // QML: loginController.createDm("@someone:server") — reuses an existing DM with
        // that person if there is one, so you never end up with five duplicate threads.
        #[qinvokable]
        #[cxx_name = "createDm"]
        fn create_dm(self: Pin<&mut LoginController>, mxid: &QString);
        // QML: loginController.createRoom(name, topic, isPrivate, isSpace)
        // NOTE: parameter names land verbatim in the generated C++ header, so they must
        // not be C++ keywords — `private` here produced "expected ',' or '...' before
        // 'private'" at build time.
        #[qinvokable]
        #[cxx_name = "createRoom"]
        fn create_room(
            self: Pin<&mut LoginController>,
            name: &QString,
            topic: &QString,
            is_private: bool,
            is_space: bool,
            parent_space: &QString,
        );
        // QML: loginController.setRoomNotifyMode(roomId, "all"|"mentions"|"mute") —
        // per-room notification level. Stored as a SERVER push rule, so the web client
        // and every other device agree with what you picked here.
        #[qinvokable]
        #[cxx_name = "setRoomNotifyMode"]
        fn set_room_notify_mode(
            self: Pin<&mut LoginController>,
            room_id: &QString,
            mode: &QString,
        );
        // QML: loginController.acceptInvite(roomId) / rejectInvite(roomId)
        #[qinvokable]
        fn accept_invite(self: Pin<&mut LoginController>, room_id: &QString);
        /// QML: loginController.joinRoom(roomId) — join a room we are NOT a member of
        /// (a public/restricted child discovered in a space). Fails loudly rather than
        /// silently if the server refuses; a refusal is a real answer, not a glitch.
        #[qinvokable]
        #[cxx_name = "joinRoom"]
        fn join_room(self: Pin<&mut LoginController>, room_id: &QString);
        /// QML: loginController.joinByLink(link) — join whatever a pasted link points
        /// at (matrix.to, a web-client url, or a bare `#alias:server` / `!id:server`).
        /// Recognises guest-call links too, so pasting one gets an explanation rather
        /// than "that isn't a valid room".
        #[qinvokable]
        #[cxx_name = "joinByLink"]
        fn join_by_link(self: Pin<&mut LoginController>, link: &QString);
        /// QML: loginController.inviteToRoom(roomId, "@someone:server") — works on a
        /// SPACE as well as a room (a space is a room), so one call serves both.
        #[qinvokable]
        #[cxx_name = "inviteToRoom"]
        fn invite_to_room(self: Pin<&mut LoginController>, room_id: &QString, mxid: &QString);
        /// QML: loginController.removeFromRoom(roomId, mxid) — kick, not ban: they can
        /// be invited back. Banning stays in the room's moderation panel.
        #[qinvokable]
        #[cxx_name = "removeFromRoom"]
        fn remove_from_room(self: Pin<&mut LoginController>, room_id: &QString, mxid: &QString);
        #[qinvokable]
        fn reject_invite(self: Pin<&mut LoginController>, room_id: &QString);
    }

    // Opt in to threading so qt_thread() exists — lets async tokio tasks post
    // results back onto the Qt thread to mutate QObject properties safely.
    impl cxx_qt::Threading for LoginController {}
}

pub struct LoginControllerRust {
    homeserver: QString,
    status: QString,
    user_id: QString,
    busy: bool,
    logged_in: bool,
    verified: bool,
    backup_ready: bool,
    sync_down: bool,
    // Interactive-verification (SAS) surface — see start_device_verification.
    sas_state: QString,
    sas_emoji: QString,
    sas_peer: QString,
    sas_note: QString,
    sas_incoming: bool,
    sas_can_start: bool,
    /// The verification flow incarnation the dialog is showing (sas_driver) — its commands name it.
    sas_shown_inc: u64,
    created_room: QString,
    recovery_key: QString,
    sas_can_bootstrap: bool,
    moved_note: QString,
    moved_to: QString,
    key_first: bool,
    first_device_pending: bool,
    first_device: bool,
    server_offer_json: QString,
    rooms_json: QString,
    auto_join_room: QString,
    auto_login: QString, // 10.0.6: TINHAT_GUI_LOGIN self-test sign-in
    auto_cam: bool,
    auto_mute: bool,
    auto_open_room: QString,
    auto_fake_call: QString,
    auto_dock: QString,
    auto_theme: QString,
    saved_theme: QString,
    saved_nav_mode: QString,
    saved_tour_seen: bool,
    assets_path: QString,
    // True from login/restore success until the first sync pass completes — gates the
    // sync loop's busy/status clear so it only ends the "Loading rooms…" state and can
    // never clobber verify()'s in-flight busy/"Verifying…" (its re-entrancy guard).
    initial_load: bool,
    // end_session is under way (the call leaving, then the local part): another sign-out waits for it
    signing_out: bool,
    // Kept alive after sign-in so the rest of the app (chat/calls) can use it.
    client: Option<Client>,
    reset_stage: QString,
    reset_status: QString,
    reset_mode: QString,
    reset_note: QString,
    /// bumped by every check_password_reset / cancel: an answer for an earlier check never lands
    reset_check: u64,
    register_stage: QString,
    register_status: QString,
    /// (homeserver url, client_secret, sid) of a password reset in flight. The sid
    /// comes back from step one and is what step three proves ownership with, so it
    /// has to outlive the QML round-trip through the user's mailbox.
    pending_reset: Option<(String, String, String)>,
}

impl Default for LoginControllerRust {
    fn default() -> Self {
        Self {
            // selfhost: no server of the app's own — the one this device signed in to last (the field says
            // "yourname.tinhat.me" until then)
            homeserver: QString::from(&crate::settings::overlay_pref_read(LAST_SERVER_PREF).unwrap_or_default()),
            status: QString::default(),
            user_id: QString::default(),
            busy: false,
            logged_in: false,
            verified: false,
            backup_ready: false,
            sync_down: false,
            sas_state: QString::from(""),
            sas_emoji: QString::from(""),
            sas_peer: QString::from(""),
            sas_note: QString::from(""),
            sas_incoming: false,
            sas_can_start: false,
            sas_shown_inc: 0,
            created_room: QString::default(),
            recovery_key: QString::from(""),
            sas_can_bootstrap: false,
            moved_note: QString::default(),
            moved_to: QString::default(),
            key_first: false,
            first_device_pending: false,
            first_device: false,
            server_offer_json: QString::from(&crate::server_offer::offer_json()),
            rooms_json: QString::default(),
            // Self-test hook: when set, QML auto-opens + joins this room's call once
            // logged in (lets us verify the live render path without a manual click).
            auto_join_room: QString::from(
                std::env::var("TINHAT_GUI_AUTOJOIN").unwrap_or_default().as_str(),
            ),
            auto_login: QString::from(std::env::var("TINHAT_GUI_LOGIN").unwrap_or_default().as_str()),
            auto_cam: std::env::var_os("TINHAT_GUI_AUTOCAM").is_some(),
            auto_mute: std::env::var_os("TINHAT_GUI_AUTOMUTE").is_some(),
            auto_open_room: QString::from(
                std::env::var("TINHAT_GUI_OPENROOM").unwrap_or_default().as_str(),
            ),
            auto_fake_call: QString::from(
                std::env::var("TINHAT_GUI_FAKECALL").unwrap_or_default().as_str(),
            ),
            auto_dock: QString::from(std::env::var("TINHAT_GUI_DOCK").unwrap_or_default().as_str()),
            auto_theme: QString::from(std::env::var("TINHAT_THEME").unwrap_or_default().as_str()),
            saved_theme: QString::from(read_saved_theme().as_str()),
            saved_nav_mode: QString::from(read_saved_nav_mode().as_str()),
            saved_tour_seen: read_saved_tour_seen(),
            // QML builds "file://" + assetsPath (Main.qml furryDir/soundsDir) — expose
            // the prefixable form (plain path on unix, /C:/-style on Windows).
            assets_path: QString::from(
                crate::platform::qml_file_prefix_path(&crate::assets::assets_dir()).as_str(),
            ),
            initial_load: false,
            signing_out: false,
            client: None,
            reset_stage: QString::from(""),
            reset_status: QString::from(""),
            reset_mode: QString::from(""),
            reset_note: QString::from(""),
            reset_check: 0,
            register_stage: QString::from(""),
            register_status: QString::from(""),
            pending_reset: None,
        }
    }
}

/// A human label for a room we were invited to (RBAC §5.3 status sentences): the stripped
/// invite state carries the name / alias, and DMs resolve through the SDK's heroes.
async fn invite_room_label(room: &matrix_sdk::Room) -> String {
    match room.display_name().await {
        Ok(d) => d.to_string(),
        Err(_) => room
            .name()
            .or_else(|| room.canonical_alias().map(|a| a.to_string()))
            .unwrap_or_else(|| room.room_id().to_string()),
    }
}

/// 10.6 (REACH A1): every /sync Tin Hat runs — the live loop, the recorder, every headless
/// harness — says `set_presence=offline`, so syncing never marks the account online in
/// Synapse's own presence system (nobody opted in to that). Tin Hat's opt-in status goes
/// through `presence.rs` and the Tin Hat API instead; nothing here ever PUTs /presence.
pub fn sync_settings() -> matrix_sdk::config::SyncSettings {
    matrix_sdk::config::SyncSettings::default()
        .set_presence(matrix_sdk::ruma::presence::PresenceState::Offline)
}

/// One shared multi-thread tokio runtime for all async Matrix work.
/// `pub` so `main()` can install a runtime `EnterGuard` on the Qt main thread for
/// the whole program lifetime — without it, dropping the matrix-sdk sqlite store
/// (deadpool's `SyncWrapper::drop` calls `tokio::task::spawn_blocking().unwrap()`)
/// on a thread with no runtime context aborts the process (SIGABRT) at exit.
pub fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        // Cap async worker threads to a small fixed pool. tokio's default is one worker per
        // CPU (32 on the dev box) — wasteful for an app whose async work is almost entirely
        // network I/O (matrix-sdk sync, LiveKit signaling): a few cooperative workers juggle
        // thousands of concurrent awaits fine, and blocking work uses the SEPARATE,
        // uncapped spawn_blocking pool. `enable_all()` is REQUIRED (io + time drivers — both
        // matrix-sdk and livekit need them). Still a multi-thread `Runtime` behind the same
        // `OnceLock`, so the process-wide single-runtime + main()'s EnterGuard contract
        // (which prevents the deadpool drop-SIGABRT at teardown) is unchanged.
        // 10.8 (the CPU/leak track, notes/cpu-idle.md §3): the blocking pool is capped too. Uncapped (tokio: 512), the
        // restore burst — the store's SQLite reads, all at once — started ~70 threads 1 s after launch, and they never
        // went away: tokio wakes an idle blocking thread FIFO, so the steady trickle of store reads (tens a second in a
        // call) reaches every one of them inside its 10 s keep-alive. 32 still runs the burst ~32-wide; the work is
        // short SQLite / file / DNS jobs (none waits on another blocking job).
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .max_blocking_threads(32)
            .enable_all()
            .build()
            .expect("failed to start tokio runtime")
    })
}

/// Per-session room recency: room_id -> the sync sequence in which it last had an
/// update (timeline/state/receipts). The SDK's persisted `recency_stamp` is sparse
/// under incremental sync, so we track activity ourselves to order the room list by
/// "most recently active this session". Higher = more recent.
fn room_activity() -> &'static Mutex<HashMap<String, u64>> {
    static A: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    A.get_or_init(|| Mutex::new(HashMap::new()))
}
static SYNC_SEQ: AtomicU64 = AtomicU64::new(1);

/// Negative cache for avatar mxc that failed to fetch (404/gone/transient). Without it,
/// `resolve_avatar` re-hits the network for a broken avatar on every load_members call
/// (which runs each RoomUpdate). Entries expire after a TTL so a transiently-unavailable
/// avatar is retried later, not suppressed forever.
fn failed_avatars() -> &'static Mutex<HashMap<String, std::time::Instant>> {
    static F: OnceLock<Mutex<HashMap<String, std::time::Instant>>> = OnceLock::new();
    F.get_or_init(|| Mutex::new(HashMap::new()))
}

/// What we persist to disk to sign back in without a password.
#[derive(Serialize, Deserialize)]
struct SavedSession {
    homeserver: String,
    session: MatrixSession,
}

pub(crate) fn data_dir() -> PathBuf {
    // Platform-resolved (windows/PLATFORM_AUDIT.md): Linux = XDG_DATA_HOME|~/.local/share
    // (unchanged), Windows = %LOCALAPPDATA% (was: a cwd-relative `.local\share` because
    // HOME is unset on Windows — session/store silently landed next to wherever the exe
    // was launched from).
    crate::platform::app_data_dir()
}

fn session_file() -> PathBuf {
    data_dir().join("session.json")
}

fn store_dir() -> PathBuf {
    data_dir().join("store")
}

/// Where the chosen UI theme id is persisted (plain text: "oled", "goose", or any theme id the runtime knows).
fn theme_file() -> PathBuf {
    data_dir().join("theme")
}

/// Read the last-chosen theme, defaulting to "oled". (Rust-backed rather than QtCore
/// Settings, whose alias fought the runtime override and silently reverted.)
pub(crate) fn read_saved_theme() -> String {
    std::fs::read_to_string(theme_file())
        .map(|s| s.trim().to_string())
        .ok()
        // themes (desktop C1): any theme the runtime resolves as itself — a built-in or <data>/themes/<id>.json
        // (theme_rt.rs); anything else (the retired furry theme, a deleted theme file) falls back to OLED.
        .filter(|s| crate::theme_rt::known(s))
        .unwrap_or_else(|| "oled".to_string())
}

fn nav_mode_file() -> PathBuf {
    data_dir().join("nav_mode")
}

/// Read the saved nav mode ("persistent" = always-on Discord sidebar, the default;
/// "corner" = tuck-away hover dock for OLED burn-in avoidance).
fn read_saved_nav_mode() -> String {
    std::fs::read_to_string(nav_mode_file())
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| s == "persistent" || s == "corner")
        .unwrap_or_else(|| "persistent".to_string())
}

fn tour_seen_file() -> PathBuf {
    data_dir().join("tour_seen")
}

/// Has the first-run onboarding tour been shown? Persisted as a tiny marker file
/// (presence + "1") next to the theme/nav_mode prefs. Defaults to NOT seen, so a
/// brand-new install auto-shows the tour exactly once.
fn read_saved_tour_seen() -> bool {
    std::fs::read_to_string(tour_seen_file())
        .map(|s| s.trim() == "1")
        .unwrap_or(false)
}

/// Remove the sqlite crypto/state store. Used on a fresh login (which mints a new
/// device) and on logout, so a previous device's account left in the store can't
/// cause a "account in the store doesn't match" mismatch. Restore does NOT wipe —
/// it reuses the store for the same device.
fn wipe_store() {
    let _ = std::fs::remove_dir_all(store_dir());
}

/// This device's last server (`<config>/tinhat/ui-last-server`): the sign-in card's Server field starts there.
pub const LAST_SERVER_PREF: &str = "ui-last-server";

/// What the Server field says before anything was typed.
pub const SERVER_FIELD_HINT: &str = "Type your server's address first (like yourname.tinhat.me).";

/// The homeserver a headless harness signs in to: `TINHAT_HS` (a URL or a server name; `TINHAT_SERVER` too). No
/// default — the app has no server of its own (selfhost, 2026-10-21).
pub fn env_homeserver() -> Result<String, String> {
    for k in ["TINHAT_HS", "TINHAT_SERVER"] {
        if let Some(v) = std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) {
            return Ok(v);
        }
    }
    Err("set TINHAT_HS to your server's address (the harness has no server of its own)".to_string())
}

/// Remember the server this device signed in to (the Server field's next start).
fn remember_server(client: &Client) {
    let host = host_of(client.homeserver().as_str());
    if !host.is_empty() {
        crate::settings::overlay_pref_write(LAST_SERVER_PREF, &host);
    }
}

impl qobject::LoginController {
    pub fn login(mut self: Pin<&mut Self>, username: &QString, password: &QString) {
        if *self.busy() {
            return;
        }
        let homeserver = self.homeserver().to_string();
        if homeserver.trim().is_empty() {
            self.as_mut().set_status(QString::from(SERVER_FIELD_HINT));
            return;
        }
        let username = username.to_string();
        let password = password.to_string();

        self.as_mut().set_busy(true);
        self.as_mut().set_logged_in(false);
        self.as_mut().set_status(QString::from("Connecting…"));

        let qt_thread = self.qt_thread();
        runtime().spawn(async move {
            let result = do_login(&homeserver, &username, &password).await;
            let _ = qt_thread.queue(move |mut qobject| {
                qobject.as_mut().set_busy(false);
                match result {
                    Ok((client, user_id)) => {
                        eprintln!("[login] success: {user_id}");
                        remember_server(&client);
                        crate::session::set_client(client.clone());
                        crate::logship::note_signed_in(&user_id); // whose log this run's is (logs never cross accounts)
                        qobject.as_mut().set_user_id(QString::from(&user_id));
                        qobject.as_mut().set_status(QString::from("Signed in"));
                        qobject.as_mut().set_logged_in(true);
                        qobject.as_mut().rust_mut().client = Some(client);
                        qobject.as_mut().start_server_offer(None); // selfhost: what this server offers, now and every 10 min
                        qobject.as_mut().check_verified();
                        qobject.as_mut().refresh_rooms(); // instant: cached rooms from store
                        qobject.as_mut().set_status(QString::from("Loading rooms…"));
                        qobject.as_mut().rust_mut().initial_load = true; // first sync pass clears it
                        qobject.as_mut().start_sync(); // the sole sync -> live updates
                        qobject.as_mut().set_up_first_device(); // F1: a brand-new account's first device verifies itself
                    }
                    Err(err) => {
                        eprintln!("[login] failed: {err}");
                        qobject.as_mut().set_status(QString::from(&err));
                        qobject.as_mut().set_logged_in(false);
                    }
                }
            });
        });
    }

    /// 10.4: create an account with an invite code (a Synapse registration token), then sign in
    /// as it through the normal `login()` path so the session is built exactly like a password
    /// sign-in. Validation happens up front so the user gets a sentence, not an errcode.
    pub fn create_account(
        mut self: Pin<&mut Self>,
        username: &QString,
        password: &QString,
        invite_code: &QString,
        display_name: &QString,
    ) {
        if *self.busy() {
            return;
        }
        let homeserver = self.homeserver().to_string();
        if homeserver.trim().is_empty() {
            self.as_mut().set_register_status(QString::from(SERVER_FIELD_HINT));
            return;
        }
        let username = username.to_string().trim().to_lowercase();
        let password = password.to_string();
        let display_name = display_name.to_string().trim().to_string();
        let code = normalize_invite_code(&invite_code.to_string());
        let ok_user = !username.is_empty()
            && username.len() <= 64
            && username.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._=-/+".contains(c));
        if !ok_user {
            self.as_mut().set_register_status(QString::from(
                "Usernames are lowercase letters, numbers, dots, dashes and underscores.",
            ));
            return;
        }
        if password.chars().count() < 8 {
            self.as_mut().set_register_status(QString::from("Use a password of at least 8 characters."));
            return;
        }
        if code.is_empty() {
            self.as_mut().set_register_status(QString::from("Enter the invite code you were given."));
            return;
        }
        self.as_mut().set_busy(true);
        self.as_mut().set_register_stage(QString::default());
        self.as_mut().set_register_status(QString::from("Checking the invite code…"));

        let qt_thread = self.qt_thread();
        runtime().spawn(async move {
            let result = do_register(&homeserver, &username, &password, &code, &display_name).await;
            let _ = qt_thread.queue(move |mut qobject| {
                qobject.as_mut().set_busy(false);
                match result {
                    Ok(user_id) => {
                        eprintln!("[register] created {user_id}");
                        qobject.as_mut().set_register_stage(QString::from("created"));
                        qobject
                            .as_mut()
                            .set_register_status(QString::from("Account created — signing you in…"));
                        qobject.as_mut().login(&QString::from(&username), &QString::from(&password));
                    }
                    Err(e) => {
                        eprintln!("[register] failed: {e}");
                        qobject.as_mut().set_register_status(QString::from(&e));
                    }
                }
            });
        });
    }

    pub fn cancel_create_account(mut self: Pin<&mut Self>) {
        self.as_mut().set_register_stage(QString::default());
        self.as_mut().set_register_status(QString::default());
    }

    /// Try to restore a previously-saved session (call from QML on startup).
    pub fn restore(mut self: Pin<&mut Self>) {
        if *self.busy() || *self.logged_in() || !session_file().exists() {
            return; // nothing to do; stay on the login screen quietly
        }
        self.as_mut().set_busy(true);
        self.as_mut().set_status(QString::from("Restoring session…"));
        crate::boot::set_stage("Restoring your session", "", 55);

        let qt_thread = self.qt_thread();
        runtime().spawn(async move {
            let result = do_restore().await;
            let _ = qt_thread.queue(move |mut qobject| {
                qobject.as_mut().set_busy(false);
                match result {
                    Ok((client, user_id)) => {
                        eprintln!("[restore] success: {user_id}");
                        crate::boot::set_stage("Loading your rooms", &user_id, 80);
                        crate::session::set_client(client.clone());
                        crate::logship::note_signed_in(&user_id); // whose log this run's is (logs never cross accounts)
                        qobject.as_mut().set_user_id(QString::from(&user_id));
                        qobject.as_mut().set_status(QString::from("Signed in"));
                        qobject.as_mut().set_logged_in(true);
                        qobject.as_mut().rust_mut().client = Some(client);
                        // selfhost: what this server offers, now and every 10 min; once it has answered, the PREVIOUS
                        // session's scrubbed log goes to this server's owner (opt-out consent — no-op unless the user
                        // accepted the popup; only while the server takes reports, only if that log was this
                        // account's alone). Both platforms.
                        qobject.as_mut().start_server_offer(Some(user_id.clone()));
                        qobject.as_mut().check_verified();
                        qobject.as_mut().refresh_rooms(); // instant: cached rooms from store
                        qobject.as_mut().set_status(QString::from("Loading rooms…"));
                        qobject.as_mut().rust_mut().initial_load = true; // first sync pass clears it
                        qobject.as_mut().start_sync(); // the sole sync -> live updates
                        qobject.as_mut().set_up_first_device(); // F1: a brand-new account's first device verifies itself
                    }
                    Err(err) => {
                        eprintln!("[restore] failed: {err}");
                        crate::boot::set_stage("Please sign in", "", 95);
                        // Saved session is unusable — clear it so the user can log in fresh.
                        let _ = std::fs::remove_file(session_file());
                        // 10.1 (TH-FIRST-09): say why you are back on the sign-in card.
                        qobject.as_mut().set_status(QString::from("Your saved session couldn't be restored — please sign in again."));
                        qobject.as_mut().set_logged_in(false);
                    }
                }
            });
        });
    }

    /// Sign out (QML: the menu's Sign out, Actions "app.signOut"): end_session, the server told.
    pub fn logout(self: Pin<&mut Self>) {
        self.end_session(true, "Signed out");
    }

    /// The end of this session on this device, in Tin Hat Web's order (session.ts doLogout). First the call ends
    /// while the token still works (call_ctl::leave_for_sign_out: the call service told, mic and camera off, the
    /// membership withdrawn); a sign-out mid-call used to leave the call running, the mic live, behind the sign-in
    /// screen. Then the local part (finish_sign_out). `tell_server`: the server ends the session too, after the
    /// presence goodbyes (the device leaves the account and its token stops working; it used to stay, the token
    /// still valid); false when the server already ended it. Found by the move's desktop test (2026-10-20).
    pub fn end_session(mut self: Pin<&mut Self>, tell_server: bool, status: &str) {
        if self.rust().signing_out {
            return; // one at a time: a second click, or the sync loop's rejected token meanwhile
        }
        self.as_mut().rust_mut().signing_out = true;
        self.as_mut().set_busy(true);
        self.as_mut().set_status(QString::from("Signing out…"));
        let status = status.to_string();
        let qt = self.qt_thread();
        runtime().spawn(async move {
            crate::call_ctl::leave_for_sign_out().await; // bounded inside
            let _ = qt.queue(move |mut o| o.as_mut().finish_sign_out(tell_server, &status));
        });
    }

    /// end_session's local part, on the GUI thread once the call is over: drop the client, delete the saved session
    /// and the stores, forget everything of the account on this device (signout.rs) and, `tell_server`, end the
    /// session on the server once the presence goodbyes have gone out with its token (bounded like the web's 8 s).
    fn finish_sign_out(mut self: Pin<&mut Self>, tell_server: bool, status: &str) {
        // 10.6 (REACH §2.4 / §3.4): `DELETE /api/presence?device=this`, the last `visible:false` and
        // a pending off's `share:false` go out with THIS session's client + token, captured here.
        // 10.6.0 review: the presence engine is reset only AFTER clear_client, so its loop can never
        // pick the old client up again and hand the old account to the fresh engine.
        let old_client = crate::session::client();
        let old_user = old_client.as_ref().and_then(|c| c.user_id().map(|u| u.to_string())).unwrap_or_default();
        // The sign-in card offers the server this session was on, as Tin Hat Web's (hostOf(session.hsUrl)): after a
        // restart the field held the default, and a server that moved to its own domain says so there (check_moved,
        // run by the card each time it shows; a move ends every session).
        let old_server = old_client.as_ref().map(|c| host_of(c.homeserver().as_str())).unwrap_or_default();
        crate::nav::forget_recency();
        let _ = std::fs::remove_file(session_file());
        wipe_store();
        crate::session::clear_client();
        let goodbye = crate::presence::on_sign_out(old_client.clone());
        crate::theme_store::on_sign_out(&old_user); // themes 1b: this account's theme sync state is forgotten
        crate::layout_store::on_sign_out(); // 11.0 edit mode: the layouts' in-memory state is forgotten
        // 10.6: after clear_client (the sync task is ended, the session generation bumped): nothing
        // of this session — notification levels, pending clicks, invites, call edges — acts for the next.
        crate::notify::forget_session();
        crate::callwatch::forget_session();
        crate::server_offer::forget(); // selfhost: the next account starts without this server's answer
        crate::logship::note_signed_out(); // this run's log is nobody's alone from here on
        // …nor what the app kept of the account beside the stores: the caches of its messages and media, its lists,
        // drafts, last room, in memory too (signout.rs; the web's account keys)
        crate::signout::forget_account();
        if let (true, Some(client)) = (tell_server, old_client) {
            runtime().spawn(async move {
                if let Some(goodbye) = goodbye {
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), goodbye).await;
                }
                match tokio::time::timeout(std::time::Duration::from_secs(8), client.matrix_auth().logout()).await {
                    Ok(Ok(_)) => eprintln!("[logout] the server ended this session: the device left the account"),
                    Ok(Err(e)) => eprintln!("[logout] the server didn't end this session ({e}); signed out here all the same"),
                    Err(_) => eprintln!("[logout] the server didn't answer in 8 s; signed out here all the same"),
                }
            });
        }
        self.as_mut().rust_mut().initial_load = false;
        self.as_mut().rust_mut().client = None;
        self.as_mut().rust_mut().signing_out = false;
        self.as_mut().set_user_id(QString::default());
        self.as_mut().set_rooms_json(QString::default());
        self.as_mut().set_logged_in(false);
        self.as_mut().set_verified(false);
        self.as_mut().set_backup_ready(false);
        self.as_mut().set_sync_down(false);
        self.as_mut().set_first_device_pending(false);
        self.as_mut().set_first_device(false);
        self.as_mut().set_server_offer_json(QString::from(&crate::server_offer::offer_json()));
        self.as_mut().set_busy(false);
        self.as_mut().set_status(QString::from(status));
        if !old_server.is_empty() {
            self.as_mut().set_homeserver(QString::from(&old_server));
        }
    }

    /// selfhost: read what the server offers (server_offer.rs) now, and every 10 min while this session lasts; the
    /// QML mirror reads serverOfferJson (AppCore.mServerOffer → app.serverOn(feature)).
    /// `ship_log_for`: the restored session's account — its previous session's log ships once the server answered
    /// (so a server that takes no reports is known first).
    fn start_server_offer(mut self: Pin<&mut Self>, ship_log_for: Option<String>) {
        let Some(client) = self.rust().client.clone() else { return };
        self.as_mut().set_server_offer_json(QString::from(&crate::server_offer::offer_json()));
        let qt = self.qt_thread();
        let gen = crate::session::generation();
        runtime().spawn(async move {
            let mut first = true;
            let mut ship = ship_log_for;
            loop {
                if crate::session::generation() != gen {
                    return;
                }
                crate::server_offer::refresh(&client, first).await;
                first = false;
                if crate::session::generation() != gen {
                    return; // signed out while the server was asked: nothing of this session goes to the next
                }
                if let Some(user) = ship.take() {
                    crate::logship::ship_previous_session_log(&user);
                }
                let json = crate::server_offer::offer_json();
                let _ = qt.queue(move |mut o| {
                    if crate::session::generation() == gen && o.server_offer_json().to_string() != json {
                        o.as_mut().set_server_offer_json(QString::from(&json));
                    }
                });
                tokio::time::sleep(crate::server_offer::REFRESH).await;
            }
        });
    }

    /// QML: login.refreshServerOffer() — the window came back (as the web's visibilitychange); at most every 10 min.
    pub fn refresh_server_offer(self: Pin<&mut Self>) {
        let Some(client) = self.rust().client.clone() else { return };
        let qt = self.qt_thread();
        let gen = crate::session::generation();
        runtime().spawn(async move {
            if crate::server_offer::refresh(&client, false).await {
                let json = crate::server_offer::offer_json();
                let _ = qt.queue(move |mut o| {
                    if crate::session::generation() == gen {
                        o.as_mut().set_server_offer_json(QString::from(&json));
                    }
                });
            }
        });
    }

    /// QML: login.serverGatesJson() — the gate tables (server_offer.rs).
    pub fn server_gates_json(&self) -> QString {
        QString::from(&crate::server_offer::gates_json())
    }

    /// QML: login.serverWebUrl() — `<homeserver origin>/` of this session; "" signed out.
    pub fn server_web_url(&self) -> QString {
        QString::from(&crate::session::server_origin().filter(|o| !o.is_empty()).map(|o| format!("{o}/")).unwrap_or_default())
    }

    /// F1 (docs/VERIFY.md "First device", 2026-09-29): the first device of a brand-new account
    /// sets up the account's encryption itself instead of asking to be verified by "another device"
    /// that does not exist. For an account with no cross-signing identity and no key backup on the
    /// server, it creates the identity (this device holds the private keys: it is verified and
    /// verifies the others) plus secret storage and a key backup, and shows the new Security Key
    /// once (F2, the verify dialog's recovery-done screen). The server takes an account's FIRST
    /// cross-signing keys without a password (MSC3967, Synapse 1.145), so this asks nothing. Every
    /// other case (keys / a backup exist, the upload refused) falls through to the usual prompt.
    /// `first_device_pending` holds the verify banner and the sign-in prompt meanwhile (at most
    /// FIRST_DEVICE_MAX_WAIT_MS). Tin Hat Web twin: crypto.ts setUpFirstDevice.
    pub fn set_up_first_device(mut self: Pin<&mut Self>) {
        let Some(client) = self.rust().client.clone() else { return };
        // docs/GUESTS.md: a temporary guest account (`@guest-…`) never sets up encryption (and is never asked to verify)
        if client.user_id().is_some_and(|u| crate::server_offer::is_guest_account(u.as_str())) {
            eprintln!("[first-device] a guest account: no encryption set-up");
            return;
        }
        self.as_mut().set_first_device(false);
        self.as_mut().set_first_device_pending(true);
        let qt = self.qt_thread();
        let qt_wait = self.qt_thread();
        runtime().spawn(async move {
            let wait = crate::sas_policy::FIRST_DEVICE_MAX_WAIT_MS;
            tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
            let _ = qt_wait.queue(|mut o| o.as_mut().set_first_device_pending(false));
        });
        runtime().spawn(async move {
            let key = first_device_setup(&client).await;
            // selfhost/firstrun §3: this device set up the account's encryption and no other device exists — the
            // account's FIRST device, whose older messages can never be unlocked (utd.rs "beforeFirstSignIn")
            if key.as_ref().is_some_and(|k| !k.is_empty()) {
                if let (Some(u), Some(d)) = (client.user_id(), client.device_id()) {
                    let alone = match client.devices().await {
                        Ok(r) => r.devices.iter().all(|x| x.device_id == *d),
                        Err(e) => {
                            eprintln!("[first-device] couldn't list devices: {e}");
                            false
                        }
                    };
                    if alone {
                        crate::settings::overlay_pref_write("ui-first-device", &crate::utd::first_device_marker(u.as_str(), d.as_str()));
                    }
                }
            }
            let verified = is_device_verified(&client).await;
            let backup_ready = client.encryption().backups().are_enabled().await;
            let _ = qt.queue(move |mut o| {
                if !same_session(&client) {
                    return; // signed out / another account meanwhile
                }
                o.as_mut().set_verified(verified);
                o.as_mut().set_backup_ready(backup_ready);
                if let Some(key) = key.filter(|k| !k.is_empty()) {
                    if o.sas_state().is_empty() {
                        o.as_mut().set_first_device(true);
                        o.as_mut().set_recovery_key(QString::from(&key));
                        o.as_mut().set_sas_note(QString::from(crate::sas_policy::FIRST_DEVICE_NOTE));
                        o.as_mut().set_sas_state(QString::from("recovery-done"));
                    }
                    o.as_mut().refresh_rooms();
                }
                o.as_mut().set_first_device_pending(false);
            });
        });
    }

    /// Refresh whether THIS device is cross-signed/verified (drives the verify
    /// prompt). Internal — called after login/restore and after verify().
    pub fn check_verified(self: Pin<&mut Self>) {
        let client = match self.rust().client.clone() {
            Some(c) => c,
            None => return,
        };
        let qt = self.qt_thread();
        runtime().spawn(async move {
            let verified = is_device_verified(&client).await;
            // Diagnostics: why aren't encrypted messages decrypting?
            let bk = client.encryption().backups();
            let exists = bk.fetch_exists_on_server().await;
            let xsign = client.encryption().cross_signing_status().await;
            eprintln!(
                "[e2ee] device_verified={verified} backup_state={:?} backup_enabled={} backup_exists_on_server={:?} cross_signing={:?}",
                bk.state(),
                bk.are_enabled().await,
                exists,
                xsign,
            );
            // 10.1 (TH-FIRST-04): the sign-out dialog tells the truth about key backup.
            let backup_ready = bk.are_enabled().await;
            let key_first = !verified && account_key_first(&client).await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_verified(verified);
                o.as_mut().set_backup_ready(backup_ready);
                o.as_mut().set_key_first(key_first);
            });
        });
    }

    /// Sign-in: a server that moved to its own domain (Tin Hat Server: Move to your own domain) answers at its old
    /// name with dev.tinhat.moved_to in /.well-known/matrix/client (moved.rs parse_move). The Server field follows and
    /// the sign-in screen says why, as Tin Hat Web's LoginView does (other builds); signing in at the old name would
    /// work too (its well-known's base_url is the new address), but the app would keep the old name.
    pub fn check_moved(mut self: Pin<&mut Self>, server: &QString) {
        let host = server.to_string().trim().trim_start_matches("https://").trim_start_matches("http://")
            .trim_end_matches('/').to_ascii_lowercase();
        if host.is_empty() || host.contains('/') || host.contains(' ') {
            return;
        }
        if self.moved_to().to_string() != host {  // another server typed: the note was about the one before
            self.as_mut().set_moved_note(QString::default());
            self.as_mut().set_moved_to(QString::default());
        }
        let qt = self.qt_thread();
        runtime().spawn(async move {
            let url = format!("https://{host}/.well-known/matrix/client");
            let Ok(resp) = matrix_sdk::reqwest::Client::new().get(&url).timeout(std::time::Duration::from_secs(8)).send().await else {
                return;
            };
            if !resp.status().is_success() {
                return;
            }
            let Ok(text) = resp.text().await else { return };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return };
            let Some(to) = crate::moved::parse_move(&v).moved_to.filter(|to| *to != host) else { return };
            let _ = qt.queue(move |mut o| {
                let now = o.homeserver().to_string().trim().trim_start_matches("https://").trim_end_matches('/').to_ascii_lowercase();
                if now != host {
                    return; // typed another server meanwhile
                }
                o.as_mut().set_moved_note(QString::from(&format!("{host} moved to {to}: sign in there with the same username and password.")));
                o.as_mut().set_moved_to(QString::from(&to));
                o.as_mut().set_homeserver(QString::from(&to));
            });
        });
    }

    /// Verify this device with the user's Security Key: open secret storage,
    /// import the cross-signing + key-backup secrets, then reload rooms (now
    /// decryptable). Like Element's "Verify this device".
    pub fn verify(mut self: Pin<&mut Self>, security_key: &QString) {
        let key = security_key.to_string();
        let client = match self.rust().client.clone() {
            Some(c) => c,
            None => return,
        };
        if *self.busy() || key.trim().is_empty() {
            return;
        }
        self.as_mut().set_busy(true);
        self.as_mut().set_status(QString::from("Verifying…"));

        let qt = self.qt_thread();
        runtime().spawn(async move {
            let key = key.trim();
            // recover() imports cross-signing (+ cross-signs THIS device) and tries
            // to enable the key backup. Backup activation only happens if the server
            // backup version is visible at that instant — so if it didn't enable but
            // one exists, re-run recover() (idempotent) now that it's fetchable.
            let mut result = client.encryption().recovery().recover(key).await;
            // A moved server (moved.rs): the old identities were made for the old name and are gone, so Secret
            // Storage held no cross-signing keys to import. Make a new identity (the first upload needs no password)
            // and keep it under the same Security Key, as Tin Hat Web's bootstrapCrossSigning does.
            if result.is_ok() && !is_device_verified(&client).await && own_identity_missing(&client).await {
                if let Err(e) = new_identity_under_key(&client, key).await {
                    eprintln!("[verify] new identity after a move failed: {e}");
                } else {
                    eprintln!("[verify] no identity on the server (moved?): made a new one under the same Security Key");
                }
            }
            if result.is_ok() {
                let bk = client.encryption().backups();
                if !bk.are_enabled().await && bk.fetch_exists_on_server().await.unwrap_or(false) {
                    eprintln!("[verify] backup didn't enable; retrying recover()");
                    result = client.encryption().recovery().recover(key).await;
                }
                eprintln!(
                    "[verify] backup_enabled={} state={:?}",
                    bk.are_enabled().await,
                    bk.state()
                );
                // Pull megolm keys for all joined rooms from backup (background) so
                // history decrypts everywhere, not just the next room opened.
                if result.is_ok() {
                    let c2 = client.clone();
                    runtime().spawn(async move {
                        let bk = c2.encryption().backups();
                        let rooms = c2.joined_rooms();
                        eprintln!("[verify] downloading backup keys for {} rooms…", rooms.len());
                        for room in rooms {
                            let _ = bk.download_room_keys_for_room(room.room_id()).await;
                        }
                        eprintln!("[verify] backup key download pass complete");
                    });
                }
            }
            let verified = is_device_verified(&client).await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_busy(false);
                match result {
                    Ok(()) => {
                        eprintln!("[verify] recover OK; device verified={verified}");
                        o.as_mut().set_verified(verified);
                        o.as_mut().set_status(QString::from("Verified — decrypting…"));
                        o.as_mut().refresh_rooms(); // re-load, now decryptable
                    }
                    Err(e) => {
                        eprintln!("[verify] failed: {e}");
                        o.as_mut()
                            .set_status(QString::from(&format!("Verification failed: {e}")));
                    }
                }
            });
        });
    }

    /// Re-publish the joined-room list from the local store (instant — no network;
    /// the background sync keeps the store fresh). Used by the Refresh button.
    pub fn refresh_rooms(self: Pin<&mut Self>) {
        let Some(client) = self.rust().client.clone() else { return };
        let qt = self.qt_thread();
        runtime().spawn(async move {
            let json = rooms_json(&client).await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_rooms_json(QString::from(&json));
                o.as_mut().set_status(QString::default());
            });
        });
    }

    /// Leave a room (by id) and refresh the list. Used by the room-list context menu.
    pub fn leave_room(self: Pin<&mut Self>, room_id: &QString) {
        let Some(client) = self.rust().client.clone() else { return };
        let rid = room_id.to_string();
        let qt = self.qt_thread();
        runtime().spawn(async move {
            if let Ok(id) = matrix_sdk::ruma::RoomId::parse(&rid) {
                if let Some(room) = client.get_room(&id) {
                    if let Err(e) = room.leave().await {
                        eprintln!("[rooms] leave failed: {e}");
                    }
                }
            }
            let json = rooms_json(&client).await;
            let _ = qt.queue(move |mut o| o.as_mut().set_rooms_json(QString::from(&json)));
        });
    }

    /// Toggle the `m.favourite` room tag (favourited rooms sort to the top).
    pub fn set_favourite(self: Pin<&mut Self>, room_id: &QString, favourite: bool) {
        let Some(client) = self.rust().client.clone() else { return };
        let rid = room_id.to_string();
        let qt = self.qt_thread();
        runtime().spawn(async move {
            if let Ok(id) = matrix_sdk::ruma::RoomId::parse(&rid) {
                if let Some(room) = client.get_room(&id) {
                    use matrix_sdk::ruma::events::tag::{TagInfo, TagName};
                    let r = if favourite {
                        room.set_tag(TagName::Favorite, TagInfo::new()).await.map(|_| ())
                    } else {
                        room.remove_tag(TagName::Favorite).await.map(|_| ())
                    };
                    if let Err(e) = r {
                        eprintln!("[rooms] set_favourite failed: {e}");
                    }
                }
            }
            let json = rooms_json(&client).await;
            let _ = qt.queue(move |mut o| o.as_mut().set_rooms_json(QString::from(&json)));
        });
    }

    /// Open a direct message with someone, creating it only if one does not exist.
    ///
    /// Matrix will happily create a second, third and fourth DM with the same person,
    /// which is how clients end up with duplicate threads that split your history. So
    /// look first: any joined DM whose only other member is them gets reused.
    /// 2026-09-29: the New DM field's rule (dm_target.rs; the web's dmTarget.ts) against this account's server.
    pub fn dm_target(&self, text: &QString) -> QString {
        let own = self.user_id().to_string();
        QString::from(crate::dm_target::dm_target(&text.to_string(), crate::dm_target::server_of(&own)).unwrap_or_default().as_str())
    }

    /// 2026-10-23: New room's line (new_room.rs hint; the web's newRoomHint). `parent_space` is what QML hands
    /// createRoom: the same filter as create_room + do_create_room, so the line matches the room that gets made.
    pub fn new_room_hint(&self, is_private: bool, is_space: bool, parent_space: &QString) -> QString {
        let parent = parent_space.to_string();
        let parent = Some(parent.as_str()).filter(|p| p.starts_with('!') && matrix_sdk::ruma::RoomId::parse(*p).is_ok());
        QString::from(crate::new_room::hint(is_space, is_private, parent))
    }

    pub fn create_dm(mut self: Pin<&mut Self>, mxid: &QString) {
        let Some(client) = self.rust().client.clone() else { return };
        // a plain username works too ("blake" → @blake:<our server>, dm_target.rs); a full ID passes through
        let own = self.user_id().to_string();
        let mxid = crate::dm_target::dm_target(&mxid.to_string(), crate::dm_target::server_of(&own)).unwrap_or_else(|| mxid.to_string());
        self.as_mut().set_status(QString::from("Opening…"));
        let qt = self.qt_thread();
        runtime().spawn(async move {
            let Ok(uid) = matrix_sdk::ruma::UserId::parse(mxid.as_str()) else {
                let _ = qt.queue(|mut o| {
                    o.as_mut().set_status(QString::from("That doesn't look like a user id"))
                });
                return;
            };
            if let Some(existing) = find_existing_dm(&client, &uid).await {
                eprintln!("[create] reusing existing DM {existing}");
                let _ = qt.queue(move |mut o| {
                    o.as_mut().set_status(QString::default());
                    o.as_mut().set_created_room(QString::from(&existing));
                });
                return;
            }
            match client.create_dm(&uid).await {
                Ok(room) => {
                    let id = room.room_id().to_string();
                    eprintln!("[create] DM created {id}");
                    crate::appstate::provision_new_room(&room).await;
                    let json = rooms_json(&client).await;
                    let _ = qt.queue(move |mut o| {
                        o.as_mut().set_status(QString::default());
                        o.as_mut().set_rooms_json(QString::from(&json));
                        o.as_mut().set_created_room(QString::from(&id));
                    });
                }
                Err(e) => {
                    eprintln!("[create] DM failed: {e}");
                    let _ = qt.queue(move |mut o| {
                        o.as_mut().set_status(QString::from(&format!("Couldn't start that chat: {e}")))
                    });
                }
            }
        });
    }

    /// Create a room (or a space, which in Matrix is a room with a type).
    pub fn create_room(
        mut self: Pin<&mut Self>,
        name: &QString,
        topic: &QString,
        is_private: bool,
        is_space: bool,
        parent_space: &QString,
    ) {
        let Some(client) = self.rust().client.clone() else { return };
        let (name, topic) = (name.to_string(), topic.to_string());
        // "" (or the "home" pseudo-space) means a top-level room, exactly as before.
        let parent = parent_space.to_string();
        let parent = if parent.starts_with('!') { Some(parent) } else { None };
        if name.trim().is_empty() {
            self.as_mut().set_status(QString::from("Give it a name first"));
            return;
        }
        self.as_mut().set_status(QString::from("Creating…"));
        let qt = self.qt_thread();
        runtime().spawn(async move {
            match do_create_room(&client, &name, &topic, is_private, is_space, parent.as_deref()).await
            {
                Ok(id) => {
                    let json = rooms_json(&client).await;
                    let _ = qt.queue(move |mut o| {
                        o.as_mut().set_status(QString::default());
                        o.as_mut().set_rooms_json(QString::from(&json));
                        o.as_mut().set_created_room(QString::from(&id));
                    });
                }
                Err(e) => {
                    eprintln!("[create] room failed: {e}");
                    let _ = qt.queue(move |mut o| {
                        o.as_mut().set_status(QString::from(&format!("Couldn't create that: {e}")))
                    });
                }
            }
        });
    }

    /// Set a room's notification level. Writes a SERVER push rule (not a local
    /// preference) so the choice follows the account to web and mobile.
    pub fn set_room_notify_mode(self: Pin<&mut Self>, room_id: &QString, mode: &QString) {
        let Some(client) = self.rust().client.clone() else { return };
        let rid = room_id.to_string();
        let Some(mode) = crate::notify::mode_from_name(&mode.to_string()) else { return };
        let qt = self.qt_thread();
        runtime().spawn(async move {
            if let Ok(id) = matrix_sdk::ruma::RoomId::parse(&rid) {
                let settings = client.notification_settings().await;
                if let Err(e) = settings.set_room_notification_mode(&id, mode).await {
                    eprintln!("[notify] set_room_notification_mode failed: {e}");
                } else {
                    eprintln!("[notify] {rid} -> {}", crate::notify::mode_name(mode));
                }
            }
            let json = rooms_json(&client).await;
            let _ = qt.queue(move |mut o| o.as_mut().set_rooms_json(QString::from(&json)));
        });
    }

    /// Accept a pending invite (join the room). A failure lands in `status` as a sentence
    /// ("Couldn't join <name>: …", RBAC §5.3) so the invite card / toast can show it; success
    /// clears `status` and re-emits the room list. A DM invite then opens that DM (`dmAccepted`,
    /// selfhost/firstrun §5): whether it is one is read from our own member event BEFORE joining
    /// (m.direct only knows the room once we have joined, nav.rs `invite_is_direct`).
    pub fn accept_invite(self: Pin<&mut Self>, room_id: &QString) {
        let Some(client) = self.rust().client.clone() else { return };
        let rid = room_id.to_string();
        let qt = self.qt_thread();
        runtime().spawn(async move {
            let mut opened_dm = false;
            let msg = match matrix_sdk::ruma::RoomId::parse(&rid) {
                Ok(id) => match client.get_room(&id) {
                    Some(room) => {
                        let name = invite_room_label(&room).await;
                        let dm = crate::nav::invite_is_direct(&room).await;
                        match room.join().await {
                            Ok(()) => {
                                opened_dm = dm;
                                String::new()
                            }
                            Err(e) => {
                                eprintln!("[rooms] accept invite to {rid} failed: {e}");
                                format!("Couldn't join {name}: {e}")
                            }
                        }
                    }
                    None => "That invite isn't there any more".to_string(),
                },
                Err(e) => format!("Bad room id: {e}"),
            };
            let json = rooms_json(&client).await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_rooms_json(QString::from(&json));
                o.as_mut().set_status(QString::from(&msg));
                if opened_dm {
                    o.as_mut().dm_accepted(QString::from(&rid));
                }
            });
        });
    }

    /// Join a room we have no membership in yet — a public or space-restricted room
    /// discovered through the space hierarchy. `client.get_room()` only knows rooms we
    /// already belong to, so this joins by id instead.
    pub fn join_room(mut self: Pin<&mut Self>, room_id: &QString) {
        let Some(client) = self.rust().client.clone() else { return };
        let rid = room_id.to_string();
        self.as_mut().set_status(QString::from("Joining…"));
        let qt = self.qt_thread();
        runtime().spawn(async move {
            let msg = match matrix_sdk::ruma::RoomId::parse(&rid) {
                Ok(id) => match client.join_room_by_id(&id).await {
                    Ok(room) => {
                        eprintln!("[rooms] joined {}", room.room_id());
                        String::new()
                    }
                    Err(e) => {
                        eprintln!("[rooms] join {rid} failed: {e}");
                        "Couldn't join that room — you may need an invite.".to_string()
                    }
                },
                Err(e) => format!("Bad room id: {e}"),
            };
            let json = rooms_json(&client).await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_rooms_json(QString::from(&json));
                o.as_mut().set_status(QString::from(&msg));
            });
        });
    }

    /// Join whatever a pasted link points at. The parsing lives in room.rs and is
    /// unit-tested; this is the part that needs a homeserver.
    pub fn join_by_link(mut self: Pin<&mut Self>, link: &QString) {
        let Some(client) = self.rust().client.clone() else { return };
        let raw = link.to_string();
        let (id, via) = match crate::room::parse_join_link(&raw) {
            Ok(crate::room::JoinTarget::RoomOrAlias { id, via }) => (id, via),
            Ok(crate::room::JoinTarget::Person(who)) => {
                self.as_mut().set_status(QString::from(&format!(
                    "{who} is a person, not a space — use New chat to message them."
                )));
                return;
            }
            Err(e) => {
                self.as_mut().set_status(QString::from(&e));
                return;
            }
        };
        self.as_mut().set_status(QString::from("Joining…"));
        let qt = self.qt_thread();
        runtime().spawn(async move {
            let msg = match matrix_sdk::ruma::RoomOrAliasId::parse(&id) {
                Ok(target) => {
                    // `via` hints matter for a room whose server we do not federate with
                    // directly; without them a perfectly good invite link can 404.
                    let servers: Vec<matrix_sdk::ruma::OwnedServerName> = via
                        .iter()
                        .filter_map(|v| matrix_sdk::ruma::ServerName::parse(v).ok())
                        .collect();
                    match client.join_room_by_id_or_alias(&target, &servers).await {
                        Ok(room) => {
                            eprintln!("[rooms] joined {} from a pasted link", room.room_id());
                            String::new()
                        }
                        Err(e) => {
                            eprintln!("[rooms] join by link {id} failed: {e}");
                            "Couldn't join with that link — it may have expired, or the \
                             space may need an invite."
                                .to_string()
                        }
                    }
                }
                Err(e) => format!("That link doesn't name a valid room: {e}"),
            };
            let json = rooms_json(&client).await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_rooms_json(QString::from(&json));
                o.as_mut().set_status(QString::from(&msg));
            });
        });
    }

    /// Invite someone to any room or space by id.
    pub fn invite_to_room(mut self: Pin<&mut Self>, room_id: &QString, mxid: &QString) {
        let Some(client) = self.rust().client.clone() else { return };
        let (rid, who) = (room_id.to_string(), mxid.to_string());
        self.as_mut().set_status(QString::from("Inviting…"));
        let qt = self.qt_thread();
        runtime().spawn(async move {
            let msg = match (
                matrix_sdk::ruma::RoomId::parse(&rid),
                matrix_sdk::ruma::UserId::parse(&who),
            ) {
                (Ok(id), Ok(user)) => match client.get_room(&id) {
                    Some(room) => match room.invite_user_by_id(&user).await {
                        Ok(()) => format!("Invited {who}"),
                        Err(e) => {
                            eprintln!("[rooms] invite {who} to {rid} failed: {e}");
                            format!("Couldn't invite {who}")
                        }
                    },
                    None => "That room isn't loaded yet".to_string(),
                },
                _ => format!("{who} doesn't look like a user id"),
            };
            let _ = qt.queue(move |mut o| o.as_mut().set_status(QString::from(&msg)));
        });
    }

    /// Remove someone from any room or space by id (kick — they can be invited back).
    pub fn remove_from_room(mut self: Pin<&mut Self>, room_id: &QString, mxid: &QString) {
        let Some(client) = self.rust().client.clone() else { return };
        let (rid, who) = (room_id.to_string(), mxid.to_string());
        self.as_mut().set_status(QString::from("Removing…"));
        let qt = self.qt_thread();
        runtime().spawn(async move {
            let msg = match (
                matrix_sdk::ruma::RoomId::parse(&rid),
                matrix_sdk::ruma::UserId::parse(&who),
            ) {
                (Ok(id), Ok(user)) => match client.get_room(&id) {
                    Some(room) => match room.kick_user(&user, None).await {
                        Ok(()) => format!("Removed {who}"),
                        Err(e) => {
                            eprintln!("[rooms] remove {who} from {rid} failed: {e}");
                            format!("Couldn't remove {who}")
                        }
                    },
                    None => "That room isn't loaded yet".to_string(),
                },
                _ => format!("{who} doesn't look like a user id"),
            };
            let _ = qt.queue(move |mut o| o.as_mut().set_status(QString::from(&msg)));
        });
    }

    /// Decline a pending invite (leave the invited room). A failure lands in `status`
    /// ("Couldn't decline the invite to <name>: …", RBAC §5.3); success clears it.
    pub fn reject_invite(self: Pin<&mut Self>, room_id: &QString) {
        let Some(client) = self.rust().client.clone() else { return };
        let rid = room_id.to_string();
        let qt = self.qt_thread();
        runtime().spawn(async move {
            let msg = match matrix_sdk::ruma::RoomId::parse(&rid) {
                Ok(id) => match client.get_room(&id) {
                    Some(room) => {
                        let name = invite_room_label(&room).await;
                        match room.leave().await {
                            Ok(()) => String::new(),
                            Err(e) => {
                                eprintln!("[rooms] decline invite to {rid} failed: {e}");
                                format!("Couldn't decline the invite to {name}: {e}")
                            }
                        }
                    }
                    None => "That invite isn't there any more".to_string(),
                },
                Err(e) => format!("Bad room id: {e}"),
            };
            let json = rooms_json(&client).await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_rooms_json(QString::from(&json));
                o.as_mut().set_status(QString::from(&msg));
            });
        });
    }

    /// Persist the chosen UI theme to disk so it survives restarts (called from QML
    /// when the theme switcher changes).
    pub fn set_theme(mut self: Pin<&mut Self>, name: &QString) {
        let name = name.to_string();
        if name.is_empty() {
            return;
        }
        let _ = std::fs::create_dir_all(data_dir());
        let _ = std::fs::write(theme_file(), &name);
        self.as_mut().set_saved_theme(QString::from(&name));
        crate::colorscheme::apply(&name); // recolor the chrome immediately
    }

    pub fn set_nav_mode(mut self: Pin<&mut Self>, mode: &QString) {
        let mode = mode.to_string();
        if mode != "persistent" && mode != "corner" {
            return;
        }
        let _ = std::fs::create_dir_all(data_dir());
        let _ = std::fs::write(nav_mode_file(), &mode);
        self.as_mut().set_saved_nav_mode(QString::from(&mode));
    }

    /// Persist whether the first-run onboarding tour has been seen. Written when the
    /// user finishes/skips the tour so it never auto-shows again (the ☰ "Help & tour"
    /// item still re-opens it manually). Named distinctly from the qproperty's
    /// auto-generated `set_saved_tour_seen` setter to avoid a collision.
    pub fn set_tour_seen(mut self: Pin<&mut Self>, seen: bool) {
        let _ = std::fs::create_dir_all(data_dir());
        let _ = std::fs::write(tour_seen_file(), if seen { "1" } else { "0" });
        self.as_mut().set_saved_tour_seen(seen);
    }

    /// Activate the KDE color scheme for `theme` (recolors the whole app incl. chrome).
    pub fn apply_color_scheme(self: Pin<&mut Self>, theme: &QString) {
        crate::colorscheme::apply(&theme.to_string());
    }

    /// Start the ONE background sync (the sole sync — no `sync_once` runs elsewhere,
    /// which would deadlock the crypto-store lock). Each sync re-publishes the room
    /// list so it (and call badges) update live. Aborted on logout via `session`.
    pub fn notify_incoming_call(self: Pin<&mut Self>, room_id: &QString, room_name: &QString, who: &QString) {
        // 10.1 (TH-NOTIF-08): gated in QML by its own `notify.calls` pref, not by the mentions/DMs switch.
        let (rid, name, who) = (room_id.to_string(), room_name.to_string(), who.to_string());
        runtime().spawn(async move {
            let body = if who.is_empty() { "Someone started a call".to_string() } else { format!("{who} started a call") };
            crate::notify::notify_call(&format!("Incoming call · {name}"), &body, &rid).await;
        });
    }

    pub fn start_sync(self: Pin<&mut Self>) {
        let client = match self.rust().client.clone() {
            Some(c) => c,
            None => return,
        };
        let qt = self.qt_thread();
        crate::notify::set_login_sink(qt.clone()); // 10.0: toast actions come back through here
        let handle = runtime().spawn(async move {
            use std::time::Duration;
            crate::boot::set_stage("Catching up on messages", "", 90); // no-op once revealed
            // Manual sequential loop (NOT sync_with_callback — its nested-async
            // callback tripped a Send/trait-recursion overflow). sync_once reuses
            // the client's stored token, so each pass is an incremental sync.
            // 10.6 (REACH A1): offline presence on every pass (see sync_settings()).
            let settings = sync_settings().timeout(Duration::from_secs(30));
            let mut failures: u32 = 0; // 10.1: consecutive sync errors (3 = show "can't reach the server")
            let mut quiet_done = false; // 10.3: the quiet push rules are ensured once, after the first sync
            crate::justme_ctl::note_sync_start(); // widgets W2: "Is it just me?" reads the sync's age
            loop {
                match client.sync_once(settings.clone()).await {
                    Ok(response) => {
                        crate::justme_ctl::note_sync_ok();
                        if failures > 0 {
                            failures = 0;
                            let _ = qt.queue(|mut o| o.as_mut().set_sync_down(false));
                        }
                        if !quiet_done {
                            quiet_done = true;
                            let c = client.clone();
                            runtime().spawn(async move { crate::quietrules::ensure(&c).await; });
                            // 10.6 (REACH A5.3): the permanent "1" on spaces — once per session.
                            let c = client.clone();
                            runtime().spawn(async move { crate::nav::clear_space_receipts(&c).await; });
                        }
                        // Record which rooms saw activity this sync (for recency order).
                        let seq = SYNC_SEQ.fetch_add(1, Ordering::Relaxed);
                        {
                            let mut act = room_activity().lock().unwrap();
                            for id in response.rooms.joined.keys() {
                                act.insert(id.to_string(), seq);
                            }
                        }
                        crate::widgets_ctl::note_sync(&response); // widgets: call history + Up next follow the sync
                        crate::note_ctl::on_sync(&response); // widgets W1: a shown space's Sticky note follows the sync
                        crate::guests_ctl::on_sync(&client, &response); // selfhost: the Guests room's on/off + who waits (docs/GUESTS.md)
                        // Feed to-device events (EC peers' media keys arrive here) to
                        // the call layer. Done in the sync loop because to-device
                        // add_event_handler doesn't dispatch under manual sync_once.
                        for pe in &response.to_device {
                            use matrix_sdk::deserialized_responses::ProcessedToDeviceEvent as P;
                            // Decryption OUTCOME is the key diagnostic: a UTD from a
                            // call peer = their key reached us but we can't decrypt it.
                            let variant = match pe {
                                P::Decrypted { .. } => "decrypted",
                                P::UnableToDecrypt { .. } => "UTD",
                                P::PlainText(_) => "plain",
                                P::Invalid(_) => "invalid",
                            };
                            let raw = pe.as_raw();
                            let (ty, sender) = raw
                                .deserialize_as::<serde_json::Value>()
                                .ok()
                                .map(|v| {
                                    (
                                        v.get("type").and_then(|t| t.as_str()).unwrap_or("?").to_string(),
                                        v.get("sender").and_then(|s| s.as_str()).unwrap_or("?").to_string(),
                                    )
                                })
                                .unwrap_or_else(|| ("?".into(), "?".into()));
                            // [recv1-td] stage 1 of the receive chain: proves a peer's key
                            // event reached us AND whether Olm decrypted it. variant=UTD =>
                            // Olm/OTK/trust problem; variant=decrypted +
                            // type=io.element.call.encryption_keys => key in hand (failure is
                            // downstream). Absence => peer never targeted us (membership).
                            // Skip the chatty per-message m.room.key spam.
                            let is_room_key = ty == "m.room.key" || ty == "m.room_key";
                            if variant != "decrypted" || !is_room_key {
                                eprintln!("[recv1-td] {variant} type={ty} from {sender}");
                            }
                            // Media-key events are trusted ONLY when Olm decrypted them. A
                            // plaintext / UTD / invalid `io.element.call.encryption_keys` is
                            // unauthenticated: anyone able to send us a to-device event could
                            // otherwise plant media keys (Astra review 2026-09-21, finding 1).
                            // EC only ever sends keys encrypted, so nothing legitimate is lost;
                            // every other event type still flows through for the diagnostics.
                            // An Olm-decrypted one then goes through the key trust rules (keyrules.rs, the
                            // same as the web): claimed device = the Olm-bound device, our call's room, a
                            // joined member with a live call membership from that device, a per-device
                            // index cap. Before 12.0.0 any sender's decrypted keys went straight into the
                            // keyring — a non-member could fill its registry and make us refuse every real
                            // peer's key (rc keys-security gate, 2026-10-24).
                            if ty == crate::e2ee_keys::ENCRYPTION_KEYS_EVENT {
                                if let P::Decrypted { encryption_info, .. } = pe {
                                    if let Ok(ev) = raw.deserialize_as::<serde_json::Value>() {
                                        use matrix_sdk::deserialized_responses::AlgorithmInfo;
                                        let enc = crate::keyrules::OlmInfo {
                                            sender: encryption_info.sender.to_string(),
                                            sender_device: encryption_info.sender_device.as_ref().map(|d| d.to_string()),
                                            sender_curve: match &encryption_info.algorithm_info {
                                                AlgorithmInfo::OlmV1Curve25519AesSha2 { curve25519_public_key_base64 } => {
                                                    Some(curve25519_public_key_base64.clone())
                                                }
                                                _ => None,
                                            },
                                        };
                                        crate::key_intake::ingest(enc, ev).await;
                                    }
                                } else {
                                    eprintln!(
                                        "[recv1-td] REJECTED {variant} encryption_keys from {sender} — only Olm-decrypted key events are trusted"
                                    );
                                }
                            } else {
                                crate::call_ctl::ingest_todevice_raw(raw);
                            }

                            // Incoming device-verification request. This is
                            // handled HERE, in the sync loop, for the same reason the
                            // call layer is: add_event_handler does not dispatch under
                            // a manual sync_once loop, so an event handler for this
                            // would silently never fire and "accept on your other
                            // device" would appear to do nothing.
                            if ty == "m.key.verification.request" {
                                let txn = raw
                                    .deserialize_as::<serde_json::Value>()
                                    .ok()
                                    .and_then(|v| {
                                        v.get("content")
                                            .and_then(|c| c.get("transaction_id"))
                                            .and_then(|t| t.as_str())
                                            .map(str::to_owned)
                                    });
                                if let (Some(txn), Ok(who)) = (
                                    txn,
                                    matrix_sdk::ruma::UserId::parse(sender.as_str()),
                                ) {
                                    let c2 = client.clone();
                                    let qt2 = qt.clone();
                                    tokio::spawn(async move {
                                        let Some(req) = c2
                                            .encryption()
                                            .get_verification_request(&who, &txn)
                                            .await
                                        else {
                                            return;
                                        };
                                        // Only surface our OWN devices: this dialog is
                                        // "verify this device", not "verify a contact".
                                        if !req.is_self_verification() {
                                            return;
                                        }
                                        // S8: THE flow from the moment it arrives (like the web),
                                        // so a requester that cancels before we Accept shows as
                                        // cancelled instead of leaving a stale Accept button.
                                        crate::sas_driver::begin_incoming(
                                            req,
                                            QtSasUi::arc(qt2, c2),
                                            crate::sas_driver::Rules::Current,
                                        );
                                    });
                                }
                            }

                            // The private cross-signing keys arrive by encrypted
                            // m.secret.send gossip AFTER a successful self-verification
                            // — re-check so the verify banner/sheet clears live instead
                            // of on the next restart.
                            if ty == "m.secret.send" {
                                let _ = qt.queue(|mut o| o.as_mut().check_verified());
                            }
                        }
                        // OS desktop notifications: edge-triggered off each room's
                        // highlight_count (mention/DM). Pure-Rust, receipt-free, and
                        // debounced — fires for rooms that just crossed a highlight edge
                        // and aren't the open room. The first scan after launch seeds
                        // silently. Runs every sync (cheap: local store reads only).
                        // 10.5.1: the counts are our own decryption-aware tally of this
                        // sync's notifying events (the server's are encryption-blind and
                        // counted Element Call key events as messages).
                        crate::notify::record_sync(&response.notifications);
                        crate::mentions_ctl::record_sync(&response.notifications); // widgets W1: the Mentions list
                        crate::notify::scan_highlights(&client).await;
                        // 10.6 (REACH A5.1): a NEW invite announces itself (the first pass seeds).
                        crate::notify::scan_invites(&client).await;
                        // 10.6 (REACH §1 / §2): the DM strip's activity clock, and the presence
                        // engine's "first sync done / sync healthy / account data changed" input.
                        crate::nav::note_sync(&response);
                        crate::presence::on_sync(&client, &response.account_data).await;
                        crate::theme_store::on_sync(&client, &response.account_data).await; // themes 1b (THEMES-SYNC.md)
                        crate::layout_store::on_sync(&client, &response.account_data).await; // 11.0 edit mode (EDITMODE-UI §16)
                        // 10.0: incoming-call edges (rooms whose call started/ended this pass).
                        let edges = crate::callwatch::scan(&client).await;
                        if !edges.is_empty() {
                            let _ = qt.queue(move |mut o| {
                                for e in edges {
                                    if e.started {
                                        o.as_mut().incoming_call(QString::from(&e.room_id), QString::from(&e.room_name), QString::from(&e.who));
                                    } else {
                                        o.as_mut().incoming_call_ended(QString::from(&e.room_id));
                                    }
                                }
                            });
                        }

                        let json = rooms_json(&client).await;
                        let _ = qt.queue(move |mut o| {
                            o.as_mut().set_rooms_json(QString::from(&json));
                            // Only end the initial "Loading rooms…" state — an unconditional
                            // busy/status clear here wiped verify()'s in-flight "Verifying…"
                            // and re-armed its busy re-entrancy guard mid-recover.
                            if o.rust().initial_load {
                                o.as_mut().rust_mut().initial_load = false;
                                o.as_mut().set_busy(false);
                                o.as_mut().set_status(QString::default());
                            }
                        });
                    }
                    Err(e) => {
                        let text = e.to_string();
                        // 10.1 (TH-FIRST-03): a rejected token is not a blip — the server ended this
                        // session (a password reset with logout_devices, "sign out" from another
                        // client, an admin purge). Retrying every 3 s forever left a fully-drawn app
                        // where nothing ever arrived and nothing sent ever landed. Sign out and say why.
                        if text.contains("M_UNKNOWN_TOKEN") || text.contains("UnknownToken") {
                            eprintln!("[sync] token rejected by the server — signing out: {text}");
                            // the server ended it already: nothing to tell it (end_session's tell_server)
                            let _ = qt.queue(|mut o| {
                                o.as_mut().end_session(
                                    false,
                                    "You were signed out: this session expired or was ended from another device. Sign in again.",
                                );
                            });
                            return;
                        }
                        failures += 1;
                        eprintln!("[sync] error ({failures} in a row): {text}; retrying in 3s");
                        if failures == 3 {
                            let _ = qt.queue(|mut o| o.as_mut().set_sync_down(true));
                        }
                        tokio::time::sleep(Duration::from_secs(3)).await;
                    }
                }
            }
        });
        crate::session::set_sync_handle(handle);
    }
}

/// A homeserver URL as the sign-in card's Server field shows it: no scheme, no trailing slash (Tin Hat Web's hostOf).
fn host_of(url: &str) -> String {
    url.trim().trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/').to_string()
}

/// Build one room-list entry (recency, json) for a joined or invited room.
async fn room_entry(
    client: &Client,
    settings: &matrix_sdk::notification_settings::NotificationSettings,
    room: &matrix_sdk::Room,
    membership: &str,
) -> (Option<u64>, serde_json::Value) {
    let name = match room.display_name().await {
        Ok(d) => d.to_string(),
        Err(_) => room
            .name()
            .or_else(|| room.canonical_alias().map(|a| a.to_string()))
            .unwrap_or_else(|| room.room_id().to_string()),
    };
    let recency = room.recency_stamp().map(u64::from);
    let unread = crate::notify::badge_counts(room).await; // 2026-10-23: encrypted-room mentions (notify::badge_counts)
    // An INVITE is a DM when our own m.room.member event says is_direct (m.direct only learns about the room after we
    // join) — the room list's rule (nav.rs nav_room_entry): Ctrl+K shows a DM invite under People, where that list has it.
    let is_dm = if membership == "invite" {
        crate::nav::invite_is_direct(room).await
    } else {
        room.is_direct().await.unwrap_or(false)
    };
    let favourite = room.is_favourite();
    // Effective notification level — drives the ✓ in the room's context menu, the mute
    // glyph, and (via `muted`) whether unread counts pull the room up the list.
    let notify_mode = crate::notify::mode_in(settings, room, is_dm).await;
    let muted = matches!(
        notify_mode,
        matrix_sdk_base::notification_settings::RoomNotificationMode::Mute
    );
    // As the room list's rows (nav.rs, REACH §1.4 / A2.14): a DM knows its one other person — Ctrl+K lists your joined
    // DM partners (ACTIONS.md §15, decision 3) — and a DM without a room picture shows that person's (the palette draws
    // a room or an invite with the room list's avatar). Local store reads only.
    let (peer, peer_avatar) = if is_dm { crate::nav::dm_peer(room, membership).await } else { (String::new(), None) };
    let avatar = match room.avatar_url().or(peer_avatar) {
        Some(mxc) => resolve_avatar(client, &mxc).await.unwrap_or_default(),
        None => String::new(),
    };
    // Ctrl+K lists pending invites newest first (ACTIONS.md §4.3 lists.invite): when we were invited, in ms (0 = unknown)
    let invited_at = if membership == "invite" { crate::nav::invite_recency_ms(room).await } else { 0 };
    let mut entry = serde_json::json!({
        "id": room.room_id().to_string(),
        "name": name,
        // 9.3: Alt+Up/Down and Ctrl+K step through ROOMS — a space is a room in Matrix, but
        // opening one as a conversation shows an empty timeline.
        "space": room.is_space(),
        "call": room.has_active_room_call(),
        // 10.1 (TH-NOTIF-03): see nav.rs — muted = 0, mentions-only = highlights.
        "unread": if muted {
            0
        } else if matches!(notify_mode, matrix_sdk_base::notification_settings::RoomNotificationMode::MentionsAndKeywordsOnly) {
            unread.highlight_count
        } else {
            unread.notification_count
        },
        "highlight": unread.highlight_count > 0,
        "dm": is_dm,
        "peer": peer,
        "favourite": favourite,
        "avatar": avatar,
        "membership": membership,
        "notifyMode": crate::notify::mode_name(notify_mode),
        "muted": muted,
        "alias": room.canonical_alias().map(|a| a.to_string()).unwrap_or_default(),
        // Whether a link is enough on its own, or the person also needs an invite.
        // Told to the user rather than discovered when their friend cannot get in.
        "publicJoin": matches!(
            room.join_rule(),
            Some(
                matrix_sdk::ruma::events::room::join_rules::JoinRule::Public
                    | matrix_sdk::ruma::events::room::join_rules::JoinRule::Knock
            )
        ),
    });
    if membership == "invite" {
        entry["invitedAt"] = serde_json::json!(invited_at);
    }
    (recency, entry)
}

/// Resolve a room/user avatar mxc to a cached local `file://` url (best-effort).
/// Cached by media id with a `Path::exists` short-circuit; failures go in a TTL negative
/// cache ([`failed_avatars`]) — both keep it off the per-RoomUpdate hot path.
pub(crate) async fn resolve_avatar(
    client: &Client,
    mxc: &matrix_sdk::ruma::MxcUri,
) -> Option<String> {
    use matrix_sdk::media::{MediaFormat, MediaRequestParameters};
    use matrix_sdk::ruma::events::room::MediaSource;
    let id: String = mxc
        .as_str()
        .rsplit('/')
        .next()
        .unwrap_or("avatar")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let dir = data_dir().join("avatars");
    let path = dir.join(format!("{id}.img"));
    if path.exists() {
        crate::room::touch_mtime(&path); // keep viewed avatars warm for the cache sweep
        return Some(crate::platform::file_url(&path));
    }
    // Skip mxc we recently failed to fetch (negative cache, TTL 1h) so a broken avatar
    // doesn't re-hit the network on every load_members tick.
    let key = mxc.as_str().to_string();
    const MISS_TTL: std::time::Duration = std::time::Duration::from_secs(3600);
    if let Ok(mut map) = failed_avatars().lock() {
        // Prune expired entries while we hold the lock (the map only ever grew before);
        // whatever remains is younger than MISS_TTL.
        map.retain(|_, t| t.elapsed() < MISS_TTL);
        if map.contains_key(&key) {
            return None;
        }
    }
    let req = MediaRequestParameters {
        source: MediaSource::Plain(mxc.to_owned()),
        format: MediaFormat::File,
    };
    let bytes = match client.media().get_media_content(&req, true).await {
        Ok(b) => b,
        Err(_) => {
            if let Ok(mut map) = failed_avatars().lock() {
                map.insert(key, std::time::Instant::now());
            }
            return None;
        }
    };
    tokio::fs::create_dir_all(&dir).await.ok()?;
    tokio::fs::write(&path, &bytes).await.ok()?;
    Some(crate::platform::file_url(&path))
}

/// Serialize the joined rooms as a JSON array of {id, name, call}, sorted by
/// **recency** (most recently active first). Uses the SDK's computed
/// `display_name()` so DMs/unnamed groups show member names (not the random room id),
/// falling back to explicit name → alias → id. Async because `display_name()` may
/// resolve members; rooms are read from the local store (no network beyond what sync
/// already cached).
async fn rooms_json(client: &Client) -> String {
    let mut rooms: Vec<(Option<u64>, serde_json::Value)> = Vec::new();
    let settings = client.notification_settings().await; // once per list, not per room (notify::mode_in)
    for room in client.joined_rooms() {
        rooms.push(room_entry(client, &settings, &room, "join").await);
    }
    // Pending invites (otherwise completely invisible) — surfaced + sorted to the top.
    for room in client.invited_rooms() {
        rooms.push(room_entry(client, &settings, &room, "invite").await);
    }
    // Order (Discord's, which is what people expect): pending invites, then DMs with
    // something new, then favourites, then rooms with a live call, then anything else
    // unread, then most recently active this session, the SDK's recency stamp, and
    // finally alphabetical. Muted rooms never claim an unread slot — muting a room is
    // exactly the request to stop having it shoved at you.
    let activity = room_activity().lock().unwrap().clone();
    let name_of = |v: &serde_json::Value| v["name"].as_str().unwrap_or("").to_lowercase();
    let act_of = |v: &serde_json::Value| {
        v["id"].as_str().and_then(|id| activity.get(id)).copied().unwrap_or(0)
    };
    let unread_of = |v: &serde_json::Value| {
        !v["muted"].as_bool().unwrap_or(false) && v["unread"].as_u64().unwrap_or(0) > 0
    };
    let unread_dm_of = |v: &serde_json::Value| unread_of(v) && v["dm"].as_bool().unwrap_or(false);
    rooms.sort_by(|a, b| {
        let a_inv = a.1["membership"].as_str() == Some("invite");
        let b_inv = b.1["membership"].as_str() == Some("invite");
        let a_fav = a.1["favourite"].as_bool().unwrap_or(false);
        let b_fav = b.1["favourite"].as_bool().unwrap_or(false);
        let a_call = a.1["call"].as_bool().unwrap_or(false);
        let b_call = b.1["call"].as_bool().unwrap_or(false);
        b_inv
            .cmp(&a_inv) // pending invites first
            .then_with(|| unread_dm_of(&b.1).cmp(&unread_dm_of(&a.1))) // then new DMs
            .then_with(|| b_fav.cmp(&a_fav)) // then favourites
            .then_with(|| b_call.cmp(&a_call))
            .then_with(|| unread_of(&b.1).cmp(&unread_of(&a.1))) // then anything unread
            .then_with(|| act_of(&b.1).cmp(&act_of(&a.1)))
            .then_with(|| match (a.0, b.0) {
                (Some(x), Some(y)) => y.cmp(&x),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| name_of(&a.1).cmp(&name_of(&b.1)))
    });
    // Ctrl+K lists the pending invites newest first (ACTIONS.md §4.3 lists.invite): each invite's place in that order,
    // `inviteRank` (0 = the newest) — nav::palette_invite_order, the web's paletteInviteOrder (QuickSwitcher.qml)
    let invites: Vec<crate::nav::PaletteInvite> = rooms
        .iter()
        .filter(|(_, v)| v["membership"].as_str() == Some("invite"))
        .map(|(_, v)| crate::nav::PaletteInvite {
            id: v["id"].as_str().unwrap_or("").to_string(),
            name: v["name"].as_str().unwrap_or("").to_string(),
            at: v["invitedAt"].as_u64().unwrap_or(0),
        })
        .collect();
    for (rank, id) in crate::nav::palette_invite_order(&invites).iter().enumerate() {
        if let Some((_, v)) = rooms.iter_mut().find(|(_, v)| v["id"].as_str() == Some(id.as_str())) {
            v["inviteRank"] = serde_json::json!(rank);
        }
    }
    let arr: Vec<serde_json::Value> = rooms.into_iter().map(|(_, v)| v).collect();
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".to_string())
}

/// Headless gate for QOL Phase 2 — proves you can start a conversation, that doing it
/// twice does not fork your history, and that a new room can actually hold a call:
///   TINHAT_CREATE_TEST=1 TINHAT_USER=… TINHAT_PASS=… TINHAT_USER2=… TINHAT_PASS2=… ./tinhat
///
/// The call-permission check is the one that matters most: a hand-made Matrix room
/// defaults state_default to 50, which silently blocks members from publishing call
/// membership. The symptom is not "calls are off" — it is peers reporting "can't key
/// us" mid-call, which is exactly the bug that cost a night of debugging before.
pub async fn headless_create_test() -> bool {
    let mut fails = 0;
    let mut check = |label: &str, ok: bool| {
        eprintln!("[create] {} {label}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            fails += 1;
        }
    };
    let (Ok(user), Ok(pass)) = (std::env::var("TINHAT_USER"), std::env::var("TINHAT_PASS")) else {
        eprintln!("[create] FAIL: need TINHAT_USER + TINHAT_PASS");
        return false;
    };
    let hs = match env_homeserver() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("[create] FAIL: {e}");
            return false;
        }
    };
    let client = match headless_login(&hs, &user, &pass).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[create] FAIL: login: {e}");
            return false;
        }
    };
    crate::session::set_client(client.clone());

    // --- a plain room, and the permissions that make calls work in it ---------------
    use matrix_sdk::ruma::api::client::room::create_room;
    let mut req = create_room::v3::Request::new();
    req.name = Some("gate room".to_owned());
    req.preset = Some(create_room::v3::RoomPreset::PrivateChat);
    let room = match client.create_room(req).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[create] FAIL: create room: {e}");
            return false;
        }
    };
    check("room created", true);
    // No background sync in a headless run, so pump one pass ourselves: provisioning
    // reads power levels from the local store and would otherwise wait for a sync that
    // is never coming. The running app has its sync loop for this.
    let settings = crate::bridge::sync_settings()
        .timeout(std::time::Duration::from_secs(5));
    let _ = client.sync_once(settings.clone()).await;
    crate::appstate::provision_new_room(&room).await;
    // Provisioning writes power levels; sync again so the read-back below sees them.
    let _ = client.sync_once(settings.clone()).await;

    // Read the power levels back off the server and assert every member-state type the
    // app needs is sendable at PL 0 — including call membership.
    let pl = crate::appstate::get_state(&room, "m.room.power_levels", "").await;
    let events = pl.as_ref().and_then(|p| p.get("events")).cloned();
    let at_zero = |t: &str| {
        events
            .as_ref()
            .and_then(|e| e.get(t))
            .and_then(|v| v.as_i64())
            == Some(0)
    };
    check("call membership is sendable by members (msc3401)", at_zero("org.matrix.msc3401.call.member"));
    check("call membership is sendable by members (m.call.member)", at_zero("m.call.member"));
    for t in crate::appstate::TINHAT_MEMBER_STATE_EVENTS {
        if t.starts_with("dev.tinhat.") {
            check(&format!("{t} sendable by members"), at_zero(t));
        }
    }

    // --- a space is a room with a type ---------------------------------------------
    use matrix_sdk::ruma::room::RoomType;
    let mut sreq = create_room::v3::Request::new();
    sreq.name = Some("gate space".to_owned());
    let cc = matrix_sdk::ruma::assign!(create_room::v3::CreationContent::new(), {
        room_type: Some(RoomType::Space),
    });
    if let Ok(raw) = matrix_sdk::ruma::serde::Raw::new(&cc) {
        sreq.creation_content = Some(raw);
    }
    match client.create_room(sreq).await {
        Ok(space) => {
            let _ = client.sync_once(settings.clone()).await;
            // is_space reads the m.room.create content, so this proves the type landed.
            check("space created and reads back as a space", space.is_space());
            let _ = space.leave().await;
        }
        Err(e) => check(&format!("space created ({e})"), false),
    }

    // --- DMs: create once, then prove a second attempt REUSES it -------------------
    match (std::env::var("TINHAT_USER2"), std::env::var("TINHAT_PASS2")) {
        (Ok(u2), _) => {
            // the second account: a bare name is on the first account's server; a full id is kept
            let server = client.user_id().map(|u| u.server_name().to_string()).unwrap_or_default();
            let bare = u2.trim_start_matches('@');
            let other = if bare.contains(':') { format!("@{bare}") } else { format!("@{bare}:{server}") };
            match matrix_sdk::ruma::UserId::parse(other.as_str()) {
                Ok(uid) => {
                    let first = client.create_dm(&uid).await;
                    check("DM created", first.is_ok());
                    if let Ok(dm) = first {
                        let id = dm.room_id().to_string();
                        // The room list needs a sync pass before the lookup can see it.
                        let _ = client.sync_once(settings.clone()).await;
                        let found = find_existing_dm(&client, &uid).await;
                        check("existing DM is found again", found.as_deref() == Some(id.as_str()));
                        check(
                            "a second 'message them' reuses it instead of forking history",
                            found.is_some(),
                        );
                        let _ = dm.leave().await;
                    }
                }
                Err(e) => check(&format!("parse second user ({e})"), false),
            }
        }
        _ => eprintln!("[create] (skipped DM dedup: no TINHAT_USER2)"),
    }

    // --- room photo (QOL 2.2b) ------------------------------------------------------
    // A room could be renamed but never given a picture: the app only ever READ
    // room.avatar_url(). Prove the write lands and the room reports it back.
    let png_path = std::env::temp_dir().join("tinhat-gate-room-photo.png");
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    check("gate image written", std::fs::write(&png_path, PNG).is_ok());
    let set = crate::room::do_apply_room_photo(
        &client,
        room.room_id().as_str(),
        png_path.to_string_lossy().as_ref(),
    )
    .await;
    check("room photo set", set.is_ok());
    let mut got = None;
    for _ in 0..10 {
        let _ = client.sync_once(settings.clone()).await;
        if let Some(u) = room.avatar_url() {
            got = Some(u);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
    check("room reports a photo afterwards", got.is_some());
    check(
        "and it is an mxc url the client can fetch",
        got.map(|u| u.as_str().starts_with("mxc://")).unwrap_or(false),
    );
    // Clearing must actually clear, not silently keep the old picture.
    let cleared = crate::room::do_apply_room_photo(&client, room.room_id().as_str(), "").await;
    check("room photo cleared", cleared.is_ok());
    let mut still = true;
    for _ in 0..10 {
        let _ = client.sync_once(settings.clone()).await;
        if room.avatar_url().is_none() {
            still = false;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
    check("photo is gone after clearing", !still);
    let _ = std::fs::remove_file(&png_path);

    let _ = room.leave().await;
    eprintln!("[create] VERDICT: {}", if fails == 0 { "PASS" } else { "FAIL" });
    fails == 0
}

/// An existing 1:1 DM with `other`, if we already have one. Matches on ACTIVE members
/// (joined or invited) so a chat where they have not accepted yet still counts — creating
/// a second one while the first invite is pending is the most annoying version of this bug.
async fn find_existing_dm(client: &Client, other: &matrix_sdk::ruma::UserId) -> Option<String> {
    for room in client.joined_rooms() {
        if !room.is_direct().await.unwrap_or(false) {
            continue;
        }
        let Ok(members) = room.members_no_sync(matrix_sdk::RoomMemberships::ACTIVE).await else {
            continue;
        };
        let others: Vec<_> = members
            .iter()
            .map(|m| m.user_id())
            .filter(|u| *u != room.own_user_id())
            .collect();
        if others.len() == 1 && others[0] == other {
            return Some(room.room_id().to_string());
        }
    }
    None
}

/// Have we recovered our identity from Secure Backup (4S)? True only once the
/// cross-signing secrets are actually imported into the OlmMachine.
///
/// NOT `get_own_device().is_verified()` — that's a false positive for our OWN
/// device (it always trusts itself), so it reported "verified" before the user
/// ever ran recover(), hiding the verify UI and leaving E2EE rooms undecryptable.
/// Is `client` still the signed-in session (same account + device)? A late result of an old session must not
/// land in the next one.
fn same_session(client: &Client) -> bool {
    crate::session::client()
        .map(|c| c.user_id() == client.user_id() && c.device_id() == client.device_id())
        .unwrap_or(false)
}

/// F1 — the first-device setup (see LoginController::set_up_first_device). `None` = not a new account
/// or it failed (the usual verify prompt follows); `Some(key)` = set up, `key` = the new Security Key
/// ("" if recovery could not be created — the device is still verified; Settings → Security offers it).
async fn first_device_setup(client: &Client) -> Option<String> {
    let enc = client.encryption();
    let me = client.user_id()?.to_owned();
    let identity = match enc.request_user_identity(&me).await {
        Ok(i) => i,
        Err(e) => {
            eprintln!("[first-device] identity query failed: {e}");
            return None;
        }
    };
    let backup = match enc.backups().fetch_exists_on_server().await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[first-device] backup query failed: {e}");
            return None;
        }
    };
    // a temporary guest account (`@guest-…`, docs/GUESTS.md) never sets up the account's encryption
    let (guest, has_xs) = (crate::server_offer::is_guest_account(me.as_str()), identity.is_some());
    eprintln!("[first-device] check: guest={guest} cross_signing_on_server={has_xs} backup_on_server={backup}");
    if !crate::sas_policy::first_device_setup(guest, has_xs, backup) {
        return None;
    }
    eprintln!("[first-device] new account: setting up cross-signing + recovery on this device");
    // no password: the server accepts an account's first cross-signing keys without one (MSC3967)
    if let Err(e) = enc.bootstrap_cross_signing(None).await {
        eprintln!("[first-device] setup failed — falling back to the verify prompt: {e}");
        return None;
    }
    eprintln!("[first-device] cross-signing bootstrapped");
    match enc.recovery().enable().await {
        Ok(key) => {
            eprintln!("[first-device] recovery on; device_verified={} key=shown", is_device_verified(client).await);
            Some(key)
        }
        Err(e) => {
            eprintln!("[first-device] recovery enable failed (device stays verified): {e}");
            Some(String::new())
        }
    }
}

/// The account has no cross-signing identity on the server. Asked of the server, not the local store: right after a
/// sign-in the store may not know the identity yet, and "none" would wrongly hide "use another device" (the web's
/// userHasCrossSigningKeys queries too). A failed request counts as "has one": the usual flow.
async fn own_identity_missing(client: &Client) -> bool {
    let Some(me) = client.user_id() else { return false };
    matches!(client.encryption().request_user_identity(me).await, Ok(None))
}

/// A new cross-signing identity (this device signed by it), stored in the account's Secret Storage under the key the
/// person just typed: their Security Key keeps working for every device after this one.
async fn new_identity_under_key(client: &Client, key: &str) -> Result<(), String> {
    let store = client.encryption().secret_storage().open_secret_store(key).await.map_err(|e| e.to_string())?;
    client.encryption().bootstrap_cross_signing(None).await.map_err(|e| e.to_string())?;
    store.export_secrets().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// keyFirst (moved.rs): Secret Storage there, no identity on the server.
async fn account_key_first(client: &Client) -> bool {
    let storage = client.encryption().secret_storage().is_enabled().await.unwrap_or(false);
    crate::moved::key_first(!own_identity_missing(client).await, storage)
}

async fn is_device_verified(client: &Client) -> bool {
    match client.encryption().cross_signing_status().await {
        Some(s) => s.has_master && s.has_self_signing && s.has_user_signing,
        None => false,
    }
}

/// Create a room (or a space) and, when `parent` is given, wire it into that space.
/// Free function so the headless gate drives the SAME path the UI does — the old create
/// gate built its own request and so could never have caught a mistake in here.
/// TINHAT_SPACEROOM_TEST=1 TINHAT_USER/PASS + TINHAT_USER2/PASS2 — prove that creating a room
/// while a space is selected puts it IN that space, and that a member of the space can
/// join it with no invite. Structural checks would not catch the thing that actually
/// bites: a room that lists inside a space and then returns 403 on join.
pub async fn headless_spaceroom_test() -> bool {
    let server = match env_homeserver() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("  FAIL {e}");
            return false;
        }
    };
    let (u1, p1) = (
        std::env::var("TINHAT_USER").unwrap_or_default(),
        std::env::var("TINHAT_PASS").unwrap_or_default(),
    );
    let (u2, p2) = (
        std::env::var("TINHAT_USER2").unwrap_or_default(),
        std::env::var("TINHAT_PASS2").unwrap_or_default(),
    );
    let mut failures = 0;
    let mut check = |label: &str, ok: bool| {
        eprintln!("  {} {label}", if ok { "ok  " } else { "FAIL" });
        if !ok {
            failures += 1;
        }
    };

    let client = match headless_login(&server, &u1, &p1).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[spaceroom] login failed: {e}");
            return false;
        }
    };
    client
        .sync_once(crate::bridge::sync_settings())
        .await
        .ok();

    let space = match do_create_room(&client, "Gate Space", "", true, true, None).await {
        Ok(id) => id,
        Err(e) => {
            eprintln!("[spaceroom] space create failed: {e}");
            return false;
        }
    };
    // PUBLIC room in the space — space members should be able to walk in.
    let room = match do_create_room(&client, "Gate Room", "", false, false, Some(&space)).await {
        Ok(id) => id,
        Err(e) => {
            eprintln!("[spaceroom] room create failed: {e}");
            return false;
        }
    };
    // PRIVATE room in the SAME space — space membership must NOT be a way in.
    let secret = match do_create_room(&client, "Gate Secret", "", true, false, Some(&space)).await {
        Ok(id) => id,
        Err(e) => {
            eprintln!("[spaceroom] private room create failed: {e}");
            return false;
        }
    };

    // Ask the SERVER what it stored rather than the SDK's cache: the cache needs a live
    // sync loop (which deadlocks against provision_new_room's state polling here), and
    // what actually matters is what every other client will see.
    let hs = if server.starts_with("http") {
        server.trim_end_matches('/').to_string() + "/"
    } else {
        format!("https://{}/", server.trim_end_matches('/'))
    };
    let token = client.access_token().unwrap_or_default();

    check(
        "space lists the new room (m.space.child)",
        gate_state(&hs, &token, &space, "m.space.child", &room).await.is_some(),
    );
    check(
        "room names its parent space (m.space.parent)",
        gate_state(&hs, &token, &room, "m.space.parent", &space).await.is_some(),
    );

    let jr = gate_state(&hs, &token, &room, "m.room.join_rules", "").await;
    let restricted = jr
        .as_ref()
        .map(|v| {
            v.get("join_rule").and_then(|j| j.as_str()) == Some("restricted")
                && v.get("allow")
                    .and_then(|a| a.as_array())
                    .map(|arr| {
                        arr.iter().any(|e| {
                            e.get("type").and_then(|t| t.as_str()) == Some("m.room_membership")
                                && e.get("room_id").and_then(|r| r.as_str()) == Some(space.as_str())
                        })
                    })
                    .unwrap_or(false)
        })
        .unwrap_or(false);
    check("public room's join rule is restricted to that space", restricted);
    if !restricted {
        eprintln!("[spaceroom] join_rules actually = {jr:?}");
    }
    // 2026-10-23 (new_room.rs, option (a)): a private room is encrypted from the start; a PUBLIC room in a space isn't
    // (its members walk in and read what was said before), and neither is the space
    let megolm = |v: Option<serde_json::Value>| {
        v.and_then(|c| c.get("algorithm").and_then(|a| a.as_str()).map(str::to_owned)).as_deref() == Some("m.megolm.v1.aes-sha2")
    };
    check(
        "public room in the space is NOT encrypted (newcomers read its history)",
        gate_state(&hs, &token, &room, "m.room.encryption", "").await.is_none(),
    );
    check("private room in the space is encrypted", megolm(gate_state(&hs, &token, &secret, "m.room.encryption", "").await));
    check("the space itself is not encrypted", gate_state(&hs, &token, &space, "m.room.encryption", "").await.is_none());

    let sjr = gate_state(&hs, &token, &secret, "m.room.join_rules", "").await;
    let secret_invite = sjr
        .as_ref()
        .and_then(|v| v.get("join_rule").and_then(|j| j.as_str()))
        == Some("invite");
    check("private room in the space stays INVITE ONLY", secret_invite);
    if !secret_invite {
        eprintln!("[spaceroom] private room join_rules = {sjr:?}");
    }

    // THE POINT: someone already in the space walks into the room with NO invite.
    if u2.is_empty() || p2.is_empty() {
        eprintln!("  skip  second-user join (set TINHAT_USER2/TINHAT_PASS2)");
    } else {
        match gate_login(&server, &u2, &p2).await {
            Ok((hs2, token2, uid)) => {
                // Invited to the SPACE only.
                if let Ok(user) = matrix_sdk::ruma::UserId::parse(&uid) {
                    if let Ok(sid) = matrix_sdk::ruma::RoomId::parse(&space) {
                        if let Some(s) = client.get_room(&sid) {
                            let _ = s.invite_user_by_id(&user).await;
                        }
                    }
                }
                check(
                    "second user joined the SPACE",
                    raw_join(&hs2, &token2, &space).await,
                );
                // Never invited to the public room — the restricted rule is the way in.
                check(
                    "space member joined the PUBLIC room with no invite",
                    raw_join(&hs2, &token2, &room).await,
                );
                // THE PRIVACY ASSERTION: being in the space must NOT open the private
                // room. A pass here means the join was REFUSED.
                check(
                    "space member is REFUSED the private room",
                    !raw_join(&hs2, &token2, &secret).await,
                );
            }
            Err(e) => {
                // Report through `check` rather than touching `failures` directly: the
                // closure holds a mutable borrow that outlives this block now.
                eprintln!("[spaceroom] second user login: {e}");
                check("second user logged in", false);
            }
        }
    }

    // Discovery, from the SECOND user's point of view: they are in the space but in
    // neither room. The public one must be offered; the private one must not appear at
    // all — a space is not a directory of the private rooms inside it.
    if !u2.is_empty() && !p2.is_empty() {
        if let Ok((_hs2, token2, _uid)) = gate_login(&server, &u2, &p2).await {
            let listed = gate_hierarchy(&hs, &token2, &space).await;
            check(
                "discovery offers the PUBLIC room",
                listed.iter().any(|r| r == &room),
            );
            check(
                "discovery HIDES the private room",
                !listed.iter().any(|r| r == &secret),
            );
        }
    }

    drop(check); // release the mutable borrow of `failures` before reading it
    eprintln!(
        "[spaceroom] VERDICT: {}",
        if failures == 0 { "PASS" } else { "FAIL" }
    );
    failures == 0
}

/// Room ids this token can SEE in a space's hierarchy, filtered the way the nav filters
/// them: only children we would actually be allowed to enter.
async fn gate_hierarchy(hs: &str, token: &str, space_id: &str) -> Vec<String> {
    let url = format!(
        "{hs}_matrix/client/v1/rooms/{}/hierarchy?limit=100",
        urlencoding_encode(space_id)
    );
    let Ok(resp) = matrix_sdk::reqwest::Client::new()
        .get(&url)
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
    else {
        return Vec::new();
    };
    let Ok(text) = resp.text().await else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    v.get("rooms")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|r| {
                    matches!(
                        r.get("join_rule").and_then(|j| j.as_str()),
                        Some("public") | Some("restricted")
                    )
                })
                .filter_map(|r| r.get("room_id").and_then(|i| i.as_str()).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Read one state event straight from the homeserver.
async fn gate_state(
    hs: &str,
    token: &str,
    room_id: &str,
    event_type: &str,
    state_key: &str,
) -> Option<serde_json::Value> {
    let url = format!(
        "{hs}_matrix/client/v3/rooms/{}/state/{}/{}",
        urlencoding_encode(room_id),
        urlencoding_encode(event_type),
        urlencoding_encode(state_key)
    );
    let resp = matrix_sdk::reqwest::Client::new()
        .get(&url)
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    serde_json::from_str(&resp.text().await.ok()?).ok()
}

/// Log the gate's second user in over raw HTTP, returning (homeserver, token, user id).
/// Deliberately NOT a second matrix_sdk Client: two Clients share one sqlite crypto store
/// and Windows cannot delete a store file that is still open (learned in the profile gate).
async fn gate_login(server: &str, user: &str, pass: &str) -> Result<(String, String, String), String> {
    let hs = if server.starts_with("http") {
        server.trim_end_matches('/').to_string() + "/"
    } else {
        format!("https://{}/", server.trim_end_matches('/'))
    };
    let body = serde_json::json!({
        "type": "m.login.password",
        "identifier": { "type": "m.id.user", "user": user },
        "password": pass,
        "initial_device_display_name": "Tin Hat space gate",
    })
    .to_string();
    let resp = matrix_sdk::reqwest::Client::new()
        .post(format!("{hs}_matrix/client/v3/login"))
        .header("Content-Type", "application/json")
        .body(body)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let text = resp.text().await.map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let token = v.get("access_token").and_then(|t| t.as_str()).ok_or_else(|| text.clone())?;
    let uid = v.get("user_id").and_then(|t| t.as_str()).unwrap_or_default();
    Ok((hs, token.to_string(), uid.to_string()))
}

/// Join a room over raw HTTP as an arbitrary token holder (the gate's second user).
async fn raw_join(hs: &str, token: &str, room_id: &str) -> bool {
    let url = format!("{hs}_matrix/client/v3/join/{}", urlencoding_encode(room_id));
    match matrix_sdk::reqwest::Client::new()
        .post(&url)
        .bearer_auth(token)
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
    {
        Ok(r) => {
            let ok = r.status().is_success();
            if !ok {
                eprintln!("[spaceroom] join {room_id} -> HTTP {}", r.status());
            }
            ok
        }
        Err(e) => {
            eprintln!("[spaceroom] join {room_id} error: {e}");
            false
        }
    }
}

fn urlencoding_encode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            _ => format!("%{:02X}", c as u32),
        })
        .collect()
}

pub(crate) async fn do_create_room(
    client: &Client,
    name: &str,
    topic: &str,
    is_private: bool,
    is_space: bool,
    parent: Option<&str>,
) -> Result<String, String> {
    use matrix_sdk::ruma::api::client::room::create_room;
    use matrix_sdk::ruma::room::RoomType;
    let mut req = create_room::v3::Request::new();
    req.name = Some(name.to_string());
    if !topic.trim().is_empty() {
        req.topic = Some(topic.to_string());
    }
    req.preset = Some(if is_private {
        create_room::v3::RoomPreset::PrivateChat
    } else {
        create_room::v3::RoomPreset::PublicChat
    });
    // What the room starts with (new_room.rs, the same list as the web's newRoom.ts — shared vectors):
    // * a room is encrypted from its first second (2026-10-23: encrypted is the default
    //   for rooms) — except a PUBLIC room inside a space (option (a): its members walk in without an invite and read
    //   what was said before); a space isn't (it holds no messages); the guest room is the server's, unencrypted;
    // * a PUBLIC room inside a space lets the space's members walk in without an individual invite (the Discord
    //   behaviour, and the only case where the space widens access); a PRIVATE room stays invite-only even inside a
    //   space — joining a space must never be a skeleton key to the private rooms in it.
    // Each entry is a WHOLE event (type + state_key + content): a content-only Raw cast to the event type is silently
    // malformed. A parent that isn't a room id is left out (no join rule), as before.
    let space = parent.filter(|p| matrix_sdk::ruma::RoomId::parse(*p).is_ok());
    for ev in crate::new_room::initial_state(is_space, is_private, space) {
        match serde_json::value::to_raw_value(&ev) {
            Ok(raw) => req.initial_state.push(matrix_sdk::ruma::serde::Raw::from_json(raw)),
            Err(e) => eprintln!("[create] initial_state {}: {e}", ev["type"]),
        }
    }
    match (space, is_space, is_private) {
        (Some(s), false, false) => eprintln!("[create] public room in space {s}: space members may join"),
        (Some(_), false, true) => eprintln!("[create] private room in a space: staying invite-only"),
        _ => {}
    }
    if is_space {
        let cc = matrix_sdk::ruma::assign!(create_room::v3::CreationContent::new(), {
            room_type: Some(RoomType::Space),
        });
        match matrix_sdk::ruma::serde::Raw::new(&cc) {
            Ok(raw) => req.creation_content = Some(raw),
            Err(e) => eprintln!("[create] space creation_content: {e}"),
        }
    }
    let room = client.create_room(req).await.map_err(|e| e.to_string())?;
    let id = room.room_id().to_string();
    eprintln!("[create] {} created {id}", if is_space { "space" } else { "room" });
    // Apply the member-state template immediately, so calls and the extras (BRB, status,
    // watch party, soundboard) work from the first second rather than after a 403.
    crate::appstate::provision_new_room(&room).await;
    if let Some(space_id) = parent {
        attach_to_space(client, space_id, &id).await;
    }
    Ok(id)
}

/// Put a freshly-created room inside a space: `m.space.child` on the space (what makes
/// it list there) plus `m.space.parent` on the room (what lets the room name its own
/// parent). Both are needed — the restricted join rule is checked against the parent.
async fn attach_to_space(client: &matrix_sdk::Client, space_id: &str, room_id: &str) {
    let (Ok(sid), Ok(rid)) = (
        matrix_sdk::ruma::RoomId::parse(space_id),
        matrix_sdk::ruma::RoomId::parse(room_id),
    ) else {
        eprintln!("[create] bad ids for space attach: {space_id} / {room_id}");
        return;
    };
    let server = rid.server_name().map(|s| s.to_string()).unwrap_or_default();
    if let Some(space) = client.get_room(&sid) {
        let content = serde_json::json!({ "via": [server], "suggested": false });
        if let Err(e) = space.send_state_event_raw("m.space.child", room_id, content).await {
            eprintln!("[create] m.space.child failed: {e}");
        }
    }
    if let Some(room) = client.get_room(&rid) {
        let sserver = sid.server_name().map(|s| s.to_string()).unwrap_or_default();
        let content = serde_json::json!({ "via": [sserver], "canonical": true });
        if let Err(e) = room.send_state_event_raw("m.space.parent", space_id, content).await {
            eprintln!("[create] m.space.parent failed: {e}");
        }
    }
    eprintln!("[create] attached {room_id} to space {space_id}");
}

async fn do_login(server: &str, username: &str, password: &str) -> Result<(Client, String), String> {
    // Fresh login = new device; start from a clean store so it can't collide with
    // a previous device's account left behind in the sqlite store.
    wipe_store();
    let client = build_client(server.trim()).await?;

    client
        .matrix_auth()
        .login_username(username, password)
        .initial_device_display_name(device_display_name())
        .await
        .map_err(|e| friendly_login_error(&e.to_string()))?;

    if let Some(session) = client.matrix_auth().session() {
        if let Err(e) = save_session(&client, &session) {
            eprintln!("[login] warning: could not save session: {e}");
        }
    }

    let user_id = client
        .user_id()
        .map(|u| u.to_string())
        .unwrap_or_else(|| username.to_string());
    Ok((client, user_id))
}

/// 10.4 (only the last blip is typed): the sign-in field shows a fixed "TH-" and people type
/// the 8 characters after it, in any case, with or without the hyphen. A pasted full code and the
/// older long tokens (`<prefix>-invite-…`) are taken as they are.
pub fn normalize_invite_code(typed: &str) -> String {
    let t = typed.trim();
    let plain = t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let bare: String = t.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_uppercase();
    if plain && bare.len() == 10 && bare.starts_with("TH") {
        return format!("TH-{}-{}", &bare[2..6], &bare[6..10]);
    }
    if plain && bare.len() == 8 {
        return format!("TH-{}-{}", &bare[..4], &bare[4..]);
    }
    t.to_string()
}

#[cfg(test)]
mod invite_code_tests {
    use super::normalize_invite_code as n;

    #[test]
    fn blip_forms_become_the_full_code() {
        assert_eq!(n("ud4d-7g3q"), "TH-UD4D-7G3Q");
        assert_eq!(n("UD4D7G3Q"), "TH-UD4D-7G3Q");
        assert_eq!(n("  ud4d7g3q "), "TH-UD4D-7G3Q");
    }

    #[test]
    fn pasted_full_codes_keep_working() {
        assert_eq!(n("TH-UD4D-7G3Q"), "TH-UD4D-7G3Q");
        assert_eq!(n("th-ud4d-7g3q"), "TH-UD4D-7G3Q");
        assert_eq!(n("thud4d7g3q"), "TH-UD4D-7G3Q");
    }

    #[test]
    fn other_tokens_pass_through_untouched() {
        assert_eq!(n("example-invite-b3a246"), "example-invite-b3a246");
        assert_eq!(n(" some.token_value "), "some.token_value");
        assert_eq!(n(""), "");
    }
}

/// 10.4 (docs/INVITES.md): register `username` with a registration token, walking the UIAA flow
/// the server offers (`m.login.registration_token`, then `m.login.dummy`). The one-shot access
/// token the server hands back only sets the display name and is logged out again — the real
/// session comes from the normal `do_login` afterwards. Returns the new user id.
async fn do_register(
    server: &str,
    username: &str,
    password: &str,
    code: &str,
    display_name: &str,
) -> Result<String, String> {
    let base = resolve_homeserver(server.trim()).await?.trim_end_matches('/').to_string();
    // The reqwest re-exported by matrix-sdk has neither the `json` nor the `query` helpers on.
    fn enc(s: &str) -> String {
        let mut out = String::with_capacity(s.len() * 3);
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }
    async fn body_json(r: matrix_sdk::reqwest::Response) -> serde_json::Value {
        serde_json::from_str(&r.text().await.unwrap_or_default()).unwrap_or(serde_json::Value::Null)
    }
    let http = matrix_sdk::reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let net = |e: matrix_sdk::reqwest::Error| {
        if e.is_timeout() {
            "The server didn't answer in time.".to_string()
        } else {
            format!("Couldn't reach {server}: {e}")
        }
    };
    // 1. The code — a clear sentence before anything else is typed for nothing.
    let v = http
        .get(format!("{base}/_matrix/client/v1/register/m.login.registration_token/validity?token={}", enc(code)))
        .send()
        .await
        .map_err(net)?;
    let vs = v.status().as_u16();
    let vj = body_json(v).await;
    if vs == 200 && vj.get("valid").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err("That invite code isn't valid — it may have expired or been used up. Ask whoever invited you for a new one.".to_string());
    }
    if vs == 403 {
        return Err("This server isn't taking new accounts right now.".to_string());
    }
    // 2. The username.
    let a = http
        .get(format!("{base}/_matrix/client/v3/register/available?username={}", enc(username)))
        .send()
        .await
        .map_err(net)?;
    if a.status().as_u16() == 400 {
        let aj = body_json(a).await;
        return Err(match aj.get("errcode").and_then(serde_json::Value::as_str).unwrap_or("") {
            "M_USER_IN_USE" => "That username is taken — try another.".to_string(),
            "M_INVALID_USERNAME" => "That username isn't allowed — use lowercase letters, numbers, dots, dashes and underscores.".to_string(),
            "M_EXCLUSIVE" => "That username is reserved on this server.".to_string(),
            _ => aj
                .get("error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("The server refused that username.")
                .to_string(),
        });
    }
    // 3. The UIAA dance: token stage, then whatever else the flow wants (dummy).
    let mut auth: Option<serde_json::Value> = None;
    let mut session: Option<String> = None;
    for _ in 0..4 {
        let mut body = serde_json::json!({
            "username": username,
            "password": password,
            "initial_device_display_name": device_display_name(),
            "inhibit_login": false,
        });
        if let Some(a) = &auth {
            body["auth"] = a.clone();
        }
        let r = http
            .post(format!("{base}/_matrix/client/v3/register"))
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(net)?;
        let status = r.status().as_u16();
        let j = body_json(r).await;
        if status == 200 {
            let user_id = j.get("user_id").and_then(serde_json::Value::as_str).unwrap_or("").to_string();
            let token = j.get("access_token").and_then(serde_json::Value::as_str).unwrap_or("").to_string();
            if !token.is_empty() {
                if !display_name.is_empty() && !user_id.is_empty() {
                    let _ = http
                        .put(format!("{base}/_matrix/client/v3/profile/{}/displayname", enc(&user_id)))
                        .bearer_auth(&token)
                        .header("Content-Type", "application/json")
                        .body(serde_json::json!({ "displayname": display_name }).to_string())
                        .send()
                        .await;
                }
                let _ = http
                    .post(format!("{base}/_matrix/client/v3/logout"))
                    .bearer_auth(&token)
                    .send()
                    .await;
            }
            return Ok(user_id);
        }
        let errcode = j.get("errcode").and_then(serde_json::Value::as_str).unwrap_or("").to_string();
        let msg = j.get("error").and_then(serde_json::Value::as_str).unwrap_or("").to_string();
        if status == 401 && errcode.is_empty() {
            // The server's "what next": remember the session, pick the next stage of a flow we can do.
            let sess = j
                .get("session")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .or_else(|| session.clone())
                .ok_or_else(|| "The server started a sign-up flow Tin Hat doesn't understand.".to_string())?;
            session = Some(sess.clone());
            let completed: Vec<String> = j
                .get("completed")
                .and_then(serde_json::Value::as_array)
                .map(|a| a.iter().filter_map(serde_json::Value::as_str).map(str::to_string).collect())
                .unwrap_or_default();
            let flows = j.get("flows").and_then(serde_json::Value::as_array).cloned().unwrap_or_default();
            let mut next: Option<String> = None;
            for f in &flows {
                let stages: Vec<&str> = f
                    .get("stages")
                    .and_then(serde_json::Value::as_array)
                    .map(|a| a.iter().filter_map(serde_json::Value::as_str).collect())
                    .unwrap_or_default();
                if stages.iter().all(|s| *s == "m.login.registration_token" || *s == "m.login.dummy") {
                    next = stages.iter().find(|s| !completed.iter().any(|c| c == *s)).map(|s| s.to_string());
                    if next.is_some() {
                        break;
                    }
                }
            }
            let Some(stage) = next else {
                return Err("This server wants an email check or a CAPTCHA to sign up, which Tin Hat can't do yet — create the account on the website, then sign in here.".to_string());
            };
            auth = Some(if stage == "m.login.registration_token" {
                serde_json::json!({ "type": stage, "token": code, "session": sess })
            } else {
                serde_json::json!({ "type": "m.login.dummy", "session": sess })
            });
            continue;
        }
        return Err(match errcode.as_str() {
            "M_USER_IN_USE" => "That username is taken — try another.".to_string(),
            "M_INVALID_USERNAME" => "That username isn't allowed — use lowercase letters, numbers, dots, dashes and underscores.".to_string(),
            "M_WEAK_PASSWORD" | "M_PASSWORD_TOO_SHORT" | "M_PASSWORD_NO_DIGIT" | "M_PASSWORD_NO_SYMBOL"
            | "M_PASSWORD_NO_UPPERCASE" | "M_PASSWORD_NO_LOWERCASE" | "M_PASSWORD_IN_DICTIONARY" => {
                if msg.is_empty() { "Pick a stronger password.".to_string() } else { format!("Pick a stronger password: {msg}") }
            }
            "M_UNAUTHORIZED" => "That invite code was refused — it may have just been used up.".to_string(),
            "M_FORBIDDEN" => "This server isn't taking new accounts right now.".to_string(),
            "M_LIMIT_EXCEEDED" => "Too many tries — wait a minute and try again.".to_string(),
            _ => {
                if msg.is_empty() { format!("The server said {status}.") } else { msg }
            }
        });
    }
    Err("The sign-up didn't finish — try again.".to_string())
}

/// Headless: restore the saved session and return the logged-in client, for
/// diagnostic/self-test entry points in `main()` (e.g. the E2EE loopback). Does NOT
/// start the background sync.
pub async fn restore_client() -> Result<Client, String> {
    let (client, _user) = do_restore().await?;
    Ok(client)
}

/// Headless: fresh password login (new device), for the TINHAT_SASTEST harness. Same
/// wipe/build/save sequence as do_login, plus TINHAT_DEVICE_ID to pin the device id —
/// SAS start-race tests need deterministic lexicographic ordering between the two
/// harness devices (the spec's race winner is the smaller device id). Does NOT start
/// the background sync.
pub async fn headless_login(server: &str, user: &str, pass: &str) -> Result<Client, String> {
    wipe_store();
    let client = build_client(server.trim()).await?;
    let mut b = client
        .matrix_auth()
        .login_username(user, pass)
        .initial_device_display_name(device_display_name());
    let dev = std::env::var("TINHAT_DEVICE_ID").ok();
    if let Some(d) = dev.as_deref() {
        b = b.device_id(d);
    }
    b.await.map_err(|e| friendly_login_error(&e.to_string()))?;
    if let Some(session) = client.matrix_auth().session() {
        let _ = save_session(&client, &session);
    }
    Ok(client)
}

async fn do_restore() -> Result<(Client, String), String> {
    let data = std::fs::read_to_string(session_file()).map_err(|e| e.to_string())?;
    let saved: SavedSession = serde_json::from_str(&data).map_err(|e| e.to_string())?;

    let client = build_client(&saved.homeserver).await?;
    client
        .restore_session(saved.session)
        .await
        .map_err(|e| format!("{e}"))?;

    let user_id = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    Ok((client, user_id))
}

fn save_session(client: &Client, session: &MatrixSession) -> Result<(), String> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let saved = SavedSession {
        homeserver: client.homeserver().to_string(),
        session: session.clone(),
    };
    let path = session_file();
    std::fs::write(&path, serde_json::to_string(&saved).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    // Protect the access token: chmod 0600 on unix. On Windows %LOCALAPPDATA% is already
    // per-user ACL-protected — skipping is the accepted call (PLATFORM_AUDIT.md §11).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Build a client (with its persistent sqlite store) against the resolved homeserver.
async fn build_client(server: &str) -> Result<Client, String> {
    let homeserver_url = resolve_homeserver(server).await?;
    base_builder()?
        .homeserver_url(homeserver_url)
        .build()
        .await
        .map_err(|e| friendly_login_error(&e.to_string()))
}

/// Resolve a delegation server-name (via `.well-known`) or a direct host/URL into a
/// concrete homeserver URL. Discovery uses a throwaway storeless client; falls back
/// to `https://<input>` when there's no `.well-known` (a bare homeserver host).
async fn resolve_homeserver(server: &str) -> Result<String, String> {
    if server.starts_with("http://") || server.starts_with("https://") {
        return Ok(server.to_string());
    }
    if let Ok(server_name) = ServerName::parse(server) {
        if let Ok(client) = Client::builder().server_name(&server_name).build().await {
            return Ok(client.homeserver().to_string());
        }
    }
    Ok(format!("https://{server}"))
}

/// A ClientBuilder with the persistent sqlite store attached.
fn base_builder() -> Result<ClientBuilder, String> {
    use matrix_sdk::encryption::{BackupDownloadStrategy, EncryptionSettings};
    std::fs::create_dir_all(data_dir()).map_err(|e| format!("data dir: {e}"))?;
    Ok(Client::builder()
        .sqlite_store(store_dir(), None)
        // Pull megolm room keys from key backup when decryption fails, so encrypted
        // history decrypts once the device is verified (default = never download).
        .with_encryption_settings(EncryptionSettings {
            auto_enable_cross_signing: false,
            backup_download_strategy: BackupDownloadStrategy::AfterDecryptionFailure,
            auto_enable_backups: false,
        }))
}

// ===================== Interactive device verification (SAS) =====================
//
// The Security Key path (verify() above) asks the user to fetch a recovery key out of
// another client's settings, which is the step people give up on — so they run
// unverified, cannot read encrypted history, and get "was this you?" forever. This is
// the path stock Element leads with instead: two of your own sessions show the same
// seven emoji, you confirm they match, and both are verified. No key, no typing.
//
// 2026-09-25: the flow itself (start policy, start guard, SAS replacement, cancel notes,
// incoming requests driven on arrival) lives in the Qt-free sas_driver.rs, shared with the
// TINHAT_SASTEST harness and mirrored by Tin Hat Web. This file only adapts it to the
// LoginController's qproperties (QtSasUi) and wires the QML invokables to it.

/// The driver's reporting sink: every call is queued onto the Qt thread.
struct QtSasUi {
    qt: cxx_qt::CxxQtThread<qobject::LoginController>,
    client: Client,
}

impl QtSasUi {
    fn arc(qt: cxx_qt::CxxQtThread<qobject::LoginController>, client: Client) -> std::sync::Arc<dyn crate::sas_driver::SasUi> {
        std::sync::Arc::new(QtSasUi { qt, client })
    }
}

/// Queue a write for verification flow `inc` onto the Qt thread; it is DROPPED if that flow is no
/// longer the one on screen when the Qt thread runs it (the user cancelled, a newer attempt or an
/// incoming request took over) — a late write of an ended flow can never overwrite a newer one.
/// `terminal` writes (cancelled / done) apply while the flow is live; progress writes (emoji,
/// buttons, "waiting") only while it is also still open, so nothing re-opens a finished flow.
/// Applying a write also records which flow the dialog is SHOWING (`sas_shown_inc`): the dialog's
/// commands (Accept, Start, They match) name that flow, not whatever became active meanwhile.
fn queue_sas<F>(qt: &cxx_qt::CxxQtThread<qobject::LoginController>, inc: u64, terminal: bool, f: F)
where
    F: FnOnce(Pin<&mut qobject::LoginController>) + Send + 'static,
{
    let _ = qt.queue(move |mut o| {
        let ok = if terminal { crate::sas_driver::is_live(inc) } else { crate::sas_driver::is_open(inc) };
        if ok {
            o.as_mut().rust_mut().sas_shown_inc = inc;
            f(o)
        }
    });
}

impl crate::sas_driver::SasUi for QtSasUi {
    fn arrived(&self, inc: u64) {
        queue_sas(&self.qt, inc, false, |mut o| {
            o.as_mut().set_sas_incoming(true);
            o.as_mut().set_sas_emoji(QString::from("[]"));
            o.as_mut().set_sas_can_bootstrap(false);
            o.as_mut().set_sas_can_start(false);
            o.as_mut().set_sas_state(QString::from("waiting"));
            o.as_mut().set_sas_note(QString::from(crate::sas_policy::NOTE_INCOMING_ARRIVED));
        });
    }
    fn set_state(&self, inc: u64, state: &str, note: &str) {
        let terminal = state == "cancelled" || state == "done";
        let (state, note) = (state.to_string(), note.to_string());
        queue_sas(&self.qt, inc, terminal, move |mut o| {
            o.as_mut().set_sas_state(QString::from(&state));
            o.as_mut().set_sas_note(QString::from(&note));
        });
    }
    fn set_peer(&self, inc: u64, peer: &str) {
        let p = peer.to_string();
        queue_sas(&self.qt, inc, false, move |mut o| o.as_mut().set_sas_peer(QString::from(&p)));
    }
    fn set_can_start(&self, inc: u64, can: bool) {
        // Hiding the button is always allowed; showing it only while the flow is open.
        queue_sas(&self.qt, inc, !can, move |mut o| o.as_mut().set_sas_can_start(can));
    }
    fn show_emoji(&self, inc: u64, emoji_json: &str, note: &str) {
        let (json, note) = (emoji_json.to_string(), note.to_string());
        queue_sas(&self.qt, inc, false, move |mut o| {
            o.as_mut().set_sas_emoji(QString::from(&json));
            o.as_mut().set_sas_state(QString::from("emoji"));
            o.as_mut().set_sas_note(QString::from(&note));
        });
    }
    fn sas_done(&self, inc: u64, note: &str) {
        let (qt, client, note) = (self.qt.clone(), self.client.clone(), note.to_string());
        tokio::spawn(async move {
            let verified = is_device_verified(&client).await;
            let _ = qt.queue(move |mut o| {
                // The dialog text belongs to this flow only; the account's verified state and the
                // room refresh are true whatever flow is on screen now.
                if crate::sas_driver::is_live(inc) {
                    o.as_mut().rust_mut().sas_shown_inc = inc;
                    o.as_mut().set_sas_state(QString::from("done"));
                    o.as_mut().set_sas_note(QString::from(&note));
                }
                o.as_mut().set_verified(verified);
                o.as_mut().refresh_rooms();
            });
            if !verified {
                // The SAS is done but the private cross-signing keys travel by secret gossip
                // and haven't landed yet — a one-instant snapshot left the verify banner up
                // until restart. The m.secret.send sync trigger usually beats this
                // poll; this is the backstop (2 min cap). (A verifier is verified already.)
                for _ in 0..40 {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    if is_device_verified(&client).await {
                        eprintln!("[sas] secrets landed — fully verified");
                        let _ = qt.queue(|mut o| {
                            o.as_mut().set_verified(true);
                            o.as_mut().refresh_rooms();
                        });
                        return;
                    }
                }
                eprintln!("[sas] secrets never arrived after SAS done (other device too old to gossip them?)");
            }
        });
    }
}

impl qobject::LoginController {
    /// Ask our other signed-in devices to verify this one by comparing emoji.
    pub fn start_device_verification(mut self: Pin<&mut Self>) {
        let client = match self.rust().client.clone() {
            Some(c) => c,
            None => return,
        };
        self.as_mut().set_sas_state(QString::from("waiting"));
        self.as_mut().set_sas_emoji(QString::from("[]"));
        self.as_mut().set_sas_incoming(false);
        self.as_mut().set_sas_can_bootstrap(false);
        self.as_mut().set_sas_can_start(false);
        self.as_mut()
            .set_sas_note(QString::from("Open Tin Hat on a device you're already signed in to — it will ask you to compare emoji."));
        // This attempt's flow incarnation, taken NOW on the Qt thread: a Cancel before the request
        // even exists moves it on, and the request is then withdrawn instead of driven.
        let (inc, previous) = crate::sas_driver::reserve();
        self.as_mut().rust_mut().sas_shown_inc = inc;
        let qt = self.qt_thread();
        runtime().spawn(async move {
            // Any open request of an earlier flow must be gone BEFORE we ask: matrix-sdk-crypto cancels
            // a new request at birth while an older one of this account is open (insert_request).
            previous.cancel_on_wire(false).await;
            crate::sas_driver::end_leftover().await;
            let Some(own) = client.user_id().map(|u| u.to_owned()) else { return };
            // Test/screenshot hook, unset in every real launch: TINHAT_GUI_VERIFY_DEVICE=<device id>
            // asks that ONE own device, the way Tin Hat Web's sessions list does
            // (startDeviceVerificationWith). The test accounts have no scriptable cross-signed
            // device, so the normal path stops at "None of your other devices is verified yet".
            if let Some(target) = std::env::var("TINHAT_GUI_VERIFY_DEVICE").ok().filter(|s| !s.is_empty()) {
                eprintln!("[sas] TINHAT_GUI_VERIFY_DEVICE: asking device {target} only");
                let dev: matrix_sdk::ruma::OwnedDeviceId = target.as_str().into();
                let res = match client.encryption().get_device(&own, &dev).await {
                    Ok(Some(d)) => d
                        .request_verification_with_methods(crate::sas_policy::our_methods())
                        .await
                        .map_err(|e| e.to_string()),
                    Ok(None) => Err(format!("device {target} is not known")),
                    Err(e) => Err(e.to_string()),
                };
                match res {
                    Ok(req) => {
                        eprintln!("[sas] request SENT flow_id={} (one device)", req.flow_id());
                        let ui = QtSasUi::arc(qt, client);
                        if !crate::sas_driver::begin_outgoing(req.clone(), inc, ui, crate::sas_driver::Rules::Current) {
                            eprintln!("[sas] the attempt was cancelled while asking — withdrawing the request");
                            let _ = req.cancel().await;
                        }
                    }
                    Err(e) => {
                        queue_sas(&qt, inc, true, move |mut o| {
                            o.as_mut().set_sas_state(QString::from("cancelled"));
                            o.as_mut().set_sas_note(QString::from(&format!("Could not start: {e}")));
                        });
                    }
                }
                return;
            }
            eprintln!("[sas] requesting self-verification for {own}");
            // A verification request is only delivered to devices this client KNOWS
            // about, so a stale own-device list silently sends it to nobody. Log the
            // targets — "request SENT" with an empty device list is the failure mode.
            // The SDK only sends a verification request to devices ALREADY signed by
            // our self-signing key (matrix-sdk-crypto filter_devices_to_request), which
            // is the protocol's whole point: an already-trusted device is what vouches
            // for a new one. If none of our devices is verified yet, the request is
            // dispatched to an EMPTY recipient list — it "succeeds" and reaches nobody,
            // and the user watches a spinner forever. Detect that up front and say so.
            let me = client.device_id().map(|d| d.to_owned());
            match client.encryption().get_user_devices(&own).await {
                Ok(devs) => {
                    let eligible: Vec<String> = devs
                        .devices()
                        .filter(|d| Some(d.device_id()) != me.as_deref() && d.is_cross_signed_by_owner())
                        .map(|d| d.device_id().to_string())
                        .collect();
                    let all: Vec<String> =
                        devs.devices().map(|d| d.device_id().to_string()).collect();
                    eprintln!("[sas] own devices={all:?} eligible(verified)={eligible:?}");
                    if eligible.is_empty() {
                        queue_sas(&qt, inc, true, |mut o| {
                            o.as_mut().set_sas_state(QString::from("cancelled"));
                            o.as_mut().set_sas_can_bootstrap(true);
                            o.as_mut().set_sas_note(QString::from(
                                "None of your other devices is verified yet, so there\'s nothing that can vouch for this one. \
                                 Set up encryption here with your account password — this device then becomes the one \
                                 that verifies the others.",
                            ));
                        });
                        return;
                    }
                }
                Err(e) => eprintln!("[sas] could not list own devices: {e}"),
            }
            let identity = match client.encryption().get_user_identity(&own).await {
                Ok(Some(i)) => {
                    eprintln!("[sas] own cross-signing identity found");
                    i
                }
                Ok(None) => {
                    // No cross-signing at all. Telling someone to use a Security Key
                    // here is a dead end — the account has never had one. Setting up
                    // recovery on this device is the way out, so offer that.
                    queue_sas(&qt, inc, true, |mut o| {
                        o.as_mut().set_sas_state(QString::from("cancelled"));
                        o.as_mut().set_sas_can_bootstrap(true);
                        o.as_mut().set_sas_note(QString::from(
                            "This account has no encryption set up yet, so there's nothing that can vouch for this device. \
                             Set it up here and this device becomes the one that verifies the rest.",
                        ));
                    });
                    return;
                }
                Err(e) => {
                    queue_sas(&qt, inc, true, move |mut o| {
                        o.as_mut().set_sas_state(QString::from("cancelled"));
                        o.as_mut().set_sas_note(QString::from(&format!("Could not start: {e}")));
                    });
                    return;
                }
            };
            // S4: advertise exactly [m.sas.v1] — byte-identical with Tin Hat Web and Element X.
            // (The SDK default added m.reciprocate.v1; no Tin Hat build shows or scans QR.)
            match identity.request_verification_with_methods(crate::sas_policy::our_methods()).await {
                Ok(req) => {
                    eprintln!(
                        "[sas] request SENT flow_id={} self={} we_started={}",
                        req.flow_id(),
                        req.is_self_verification(),
                        req.we_started()
                    );
                    let ui = QtSasUi::arc(qt, client);
                    if !crate::sas_driver::begin_outgoing(req.clone(), inc, ui, crate::sas_driver::Rules::Current) {
                        eprintln!("[sas] the attempt was cancelled while asking — withdrawing the request");
                        let _ = req.cancel().await;
                    }
                }
                Err(e) => {
                    queue_sas(&qt, inc, true, move |mut o| {
                        o.as_mut().set_sas_state(QString::from("cancelled"));
                        o.as_mut().set_sas_note(QString::from(&format!("Could not start: {e}")));
                    });
                }
            }
        });
    }

    /// Accept a verification request that arrived from another of our devices. The driver has
    /// been following it since it arrived (S8); accepting only sends our ready ([m.sas.v1], S4).
    pub fn accept_device_verification(mut self: Pin<&mut Self>) {
        if self.rust().client.is_none() {
            return;
        }
        self.as_mut().set_sas_note(QString::from("Accepting…"));
        let inc = self.rust().sas_shown_inc;
        runtime().spawn(async move {
            if let Err(e) = crate::sas_driver::accept(inc).await {
                crate::sas_driver::fail_flow(inc, &format!("Could not accept: {e}"));
            }
        });
    }

    /// S2 — "Start emoji comparison": the requester starts now instead of waiting for the other
    /// device (whose user picks the method). Goes through the same start guard as the auto start.
    pub fn start_emoji_comparison(mut self: Pin<&mut Self>) {
        if !*self.sas_can_start() {
            return;
        }
        self.as_mut().set_sas_can_start(false);
        let inc = self.rust().sas_shown_inc;
        runtime().spawn(async move {
            crate::sas_driver::start_now(inc).await;
        });
    }

    /// "The emoji match" — completes verification on this side, for the emoji on SCREEN: if the
    /// comparison was replaced after they were shown, nothing is confirmed (S5).
    pub fn confirm_sas_match(mut self: Pin<&mut Self>) {
        let shown = self.sas_emoji().to_string();
        let inc = self.rust().sas_shown_inc;
        self.as_mut().set_sas_state(QString::from("confirming"));
        self.as_mut()
            .set_sas_note(QString::from("Waiting for your other device to confirm…"));
        let qt = self.qt_thread();
        runtime().spawn(async move {
            match crate::sas_driver::confirm(inc, &shown).await {
                crate::sas_driver::Confirm::Sent => {}
                // The flow ended meanwhile: its own terminal write is on screen or queued. If the
                // dialog still says "confirming" once everything queued so far has applied, say
                // cancelled rather than wait forever (a later terminal write still wins).
                crate::sas_driver::Confirm::NoSas => {
                    queue_sas(&qt, inc, true, |mut o| {
                        if o.sas_state().to_string() == "confirming" {
                            o.as_mut().set_sas_state(QString::from("cancelled"));
                            o.as_mut().set_sas_note(QString::from(&crate::sas_policy::cancel_note(None)));
                        }
                    });
                }
                // Replaced after these emoji were shown: show the new ones if they are in;
                // otherwise the watcher shows them the moment the keys land.
                crate::sas_driver::Confirm::Stale(Some(json)) => {
                    queue_sas(&qt, inc, false, move |mut o| {
                        o.as_mut().set_sas_emoji(QString::from(&json));
                        o.as_mut().set_sas_state(QString::from("emoji"));
                        o.as_mut().set_sas_note(QString::from(crate::sas_policy::NOTE_EMOJI));
                    });
                }
                crate::sas_driver::Confirm::Stale(None) => {}
                crate::sas_driver::Confirm::Failed(gen, e) => {
                    crate::sas_driver::fail_confirm(inc, gen, &format!("Could not confirm: {e}"));
                }
            }
        });
    }

    /// Create recovery (and with it cross-signing) for an account that has never had
    /// it. Without this an account with no recovery is a dead end: it cannot recover
    /// (no key exists) and cannot use emoji verification either, because that needs an
    /// already-verified device to vouch and there is none. Enabling
    /// recovery also cross-signs THIS device, which breaks the deadlock.
    ///
    /// The returned key is shown ONCE and never stored — that is the whole point of it.
    pub fn set_up_recovery(mut self: Pin<&mut Self>, password: &QString) {
        let Some(client) = self.rust().client.clone() else { return };
        let password = password.to_string();
        if password.is_empty() {
            self.as_mut().set_sas_note(QString::from("Enter your account password to continue."));
            return;
        }
        self.as_mut().set_sas_state(QString::from("recovery-working"));
        self.as_mut()
            .set_sas_note(QString::from("Setting up recovery — this takes a few seconds…"));
        let qt = self.qt_thread();
        runtime().spawn(async move {
            // Cross-signing is what actually makes this device able to VOUCH for
            // others, and uploading those keys is a protected action — the server
            // demands interactive auth, which is why this needs the password.
            // Recovery::enable() alone does NOT do it (that was the false-success bug).
            if let Some(user) = client.user_id() {
                use matrix_sdk::ruma::api::client::uiaa;
                let auth = uiaa::AuthData::Password(uiaa::Password::new(
                    uiaa::UserIdentifier::from(user.to_owned()),
                    password.clone(),
                ));
                if let Err(e) = client
                    .encryption()
                    .bootstrap_cross_signing_if_needed(Some(auth))
                    .await
                {
                    eprintln!("[recovery] cross-signing bootstrap failed: {e}");
                    let _ = qt.queue(move |mut o| {
                        o.as_mut().set_sas_state(QString::from("cancelled"));
                        o.as_mut().set_sas_note(QString::from(&format!(
                            "Could not set up verification: {e}"
                        )));
                    });
                    return;
                }
                eprintln!("[recovery] cross-signing bootstrapped");
            }
            // Recovery may already exist (e.g. cross-signing was the only missing
            // half); enabling it again would fail, so only do it when it is off.
            use matrix_sdk::encryption::recovery::RecoveryState;
            if client.encryption().recovery().state() == RecoveryState::Enabled {
                let verified = is_device_verified(&client).await;
                eprintln!("[recovery] already enabled; device_verified={verified}");
                let _ = qt.queue(move |mut o| {
                    o.as_mut().set_sas_state(QString::from("recovery-done"));
                    o.as_mut().set_recovery_key(QString::from(""));
                    o.as_mut().set_sas_note(QString::from(
                        "This device is verified. Your recovery was already set up, so there's no new key to save.",
                    ));
                    o.as_mut().set_verified(verified);
                    o.as_mut().refresh_rooms();
                });
                return;
            }
            match client.encryption().recovery().enable().await {
                Ok(key) => {
                    // Report what actually happened, not what we hoped. Recovery::enable()
                    // creates the backup and the recovery key but does NOT bootstrap
                    // cross-signing — that needs bootstrap_cross_signing() with the
                    // user's password for interactive auth. Claiming "this device is
                    // verified" here was false: the server showed no master or
                    // self-signing key afterwards, and other devices still saw nothing
                    // that could vouch for it.
                    let verified = is_device_verified(&client).await;
                    eprintln!("[recovery] enable() ok; device_verified_after={verified}");
                    let _ = qt.queue(move |mut o| {
                        o.as_mut().set_recovery_key(QString::from(&key));
                        o.as_mut().set_sas_state(QString::from("recovery-done"));
                        o.as_mut().set_sas_note(QString::from(if verified {
                            "Recovery is on and this device is verified. Save this key somewhere safe — \
                             it is shown once and cannot be retrieved later."
                        } else {
                            "Recovery is on and your key backup is set up — save the key below, it is \
                             shown once and cannot be retrieved later. This device still isn't fully \
                             verified: close this and open Verify this device again to finish."
                        }));
                        o.as_mut().set_verified(verified);
                        o.as_mut().refresh_rooms();
                    });
                }
                Err(e) => {
                    eprintln!("[recovery] enable failed: {e}");
                    let _ = qt.queue(move |mut o| {
                        o.as_mut().set_sas_state(QString::from("cancelled"));
                        o.as_mut().set_sas_note(QString::from(&format!(
                            "Could not set up recovery: {e}"
                        )));
                    });
                }
            }
        });
    }

    /// "They don't match", or the user closed the dialog. Cancelling is the SAFE
    /// outcome — a mismatch means someone may be intercepting, so never treat a
    /// dismissal as success.
    pub fn cancel_device_verification(mut self: Pin<&mut Self>) {
        // End the flow NOW, on the Qt thread: any write it still has queued is dropped.
        let ended = crate::sas_driver::end_by_user();
        self.as_mut().rust_mut().sas_shown_inc = crate::sas_driver::current_inc();
        self.as_mut().set_sas_state(QString::from("cancelled"));
        // S6: the sentence for the code we send (m.user) — same as Tin Hat Web's local cancel
        self.as_mut().set_sas_note(QString::from(&crate::sas_policy::cancel_note(Some("m.user"))));
        self.as_mut().set_sas_can_start(false);
        runtime().spawn(ended.cancel_on_wire(false));
    }

    /// "They don't match" — like cancel, but tells the other device WHY (m.mismatched_sas, as
    /// Tin Hat Web does), so it can say "The emoji didn't match, so nothing was trusted."
    pub fn mismatch_device_verification(mut self: Pin<&mut Self>) {
        let ended = crate::sas_driver::end_by_user();
        self.as_mut().rust_mut().sas_shown_inc = crate::sas_driver::current_inc();
        self.as_mut().set_sas_state(QString::from("cancelled"));
        // S6: the sentence for the code we send (m.mismatched_sas) — same as Tin Hat Web's "They don't match"
        self.as_mut().set_sas_note(QString::from(&crate::sas_policy::cancel_note(Some("m.mismatched_sas"))));
        self.as_mut().set_sas_can_start(false);
        runtime().spawn(ended.cancel_on_wire(true));
    }
}

// ---------------------------------------------------------------------------
// Forgot password (unauthenticated)
//
// The account API can't be used here: the whole point is that the user cannot get
// an access token. So this talks to the homeserver's client-server endpoints
// directly over HTTP, with no session, no store, and no matrix-sdk Client —
// building one would attach the sqlite store and mint device state for someone who
// hasn't proven who they are yet.
//
// Three steps, and the middle one happens in the user's mail client:
//   1. POST /account/password/email/requestToken  -> {sid}, and a link is mailed
//   2. the user clicks the link, which validates the token server-side
//   3. POST /account/password with auth m.login.email.identity + {sid, client_secret}
//
// Step 3 is user-interactive auth: the first call is expected to come back 401 with
// a session id, and the real attempt carries it. Synapse accepts a sessionless
// attempt too, so both shapes are handled rather than assuming one.
// ---------------------------------------------------------------------------
impl qobject::LoginController {
    pub fn request_password_reset(mut self: Pin<&mut Self>, email: &QString) {
        if *self.busy() {
            return;
        }
        let email = email.to_string().trim().to_string();
        if !email.contains('@') {
            self.as_mut()
                .set_reset_status(QString::from("Enter the email address on your account."));
            return;
        }
        let homeserver = self.homeserver().to_string();
        // A fresh secret per attempt; reusing one would let the server tie this
        // request to a previous address.
        let secret = format!(
            "tinhat{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        self.as_mut().set_busy(true);
        self.as_mut()
            .set_reset_status(QString::from("Sending reset email…"));

        let qt_thread = self.as_mut().qt_thread();
        let secret_for_task = secret.clone();
        runtime().spawn(async move {
            let result = do_request_password_reset(&homeserver, &email, &secret_for_task).await;
            let _ = qt_thread.queue(move |mut qobject| {
                qobject.as_mut().set_busy(false);
                match result {
                    Ok((base, sid)) => {
                        qobject.as_mut().rust_mut().pending_reset =
                            Some((base, secret_for_task, sid));
                        qobject.as_mut().set_reset_stage(QString::from("sent"));
                        qobject.as_mut().set_reset_status(QString::from(&format!(
                            "Check {email} and click the link, then set a new password below."
                        )));
                    }
                    Err(e) => {
                        qobject.as_mut().set_reset_status(QString::from(&e));
                    }
                }
            });
        });
    }

    pub fn complete_password_reset(mut self: Pin<&mut Self>, new_password: &QString) {
        if *self.busy() {
            return;
        }
        let Some((base, secret, sid)) = self.as_mut().rust_mut().pending_reset.clone() else {
            self.as_mut()
                .set_reset_status(QString::from("Ask for a reset email first."));
            return;
        };
        let new_password = new_password.to_string();
        if new_password.len() < 8 {
            self.as_mut()
                .set_reset_status(QString::from("Use at least 8 characters."));
            return;
        }
        self.as_mut().set_busy(true);
        self.as_mut()
            .set_reset_status(QString::from("Setting your new password…"));

        let qt_thread = self.as_mut().qt_thread();
        runtime().spawn(async move {
            let result = do_complete_password_reset(&base, &secret, &sid, &new_password).await;
            let _ = qt_thread.queue(move |mut qobject| {
                qobject.as_mut().set_busy(false);
                match result {
                    Ok(()) => {
                        qobject.as_mut().rust_mut().pending_reset = None;
                        qobject.as_mut().set_reset_stage(QString::from("done"));
                        qobject.as_mut().set_reset_status(QString::from(
                            "Password changed. Sign in with your new password.",
                        ));
                    }
                    Err(e) => {
                        qobject.as_mut().set_reset_status(QString::from(&e));
                    }
                }
            });
        });
    }

    pub fn cancel_password_reset(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().pending_reset = None;
        self.as_mut().rust_mut().reset_check += 1; // a check still on its way no longer lands
        self.as_mut().set_reset_stage(QString::from(""));
        self.as_mut().set_reset_status(QString::from(""));
        self.as_mut().set_reset_mode(QString::from(""));
        self.as_mut().set_reset_note(QString::from(""));
    }

    /// selfhost (password.rs): ask the typed server how it takes back a forgotten password — `"owner"` in its
    /// well-known (`dev.tinhat.server.account.password_reset`) = no email field, the owner face; anything else = the
    /// email form. "Checking <host>…" meanwhile. TINHAT_SERVER_OFFER pins the answer (the server is never asked).
    pub fn check_password_reset(mut self: Pin<&mut Self>, server: &QString) {
        self.as_mut().rust_mut().reset_check += 1;
        let check = self.rust().reset_check;
        let Some(host) = crate::password::server_host(&server.to_string()) else {
            // nothing usable typed: the email form, as before
            self.as_mut().show_password_reset(crate::password::PasswordReset::Email, "");
            return;
        };
        if let Some(o) = crate::server_offer::pinned_offer() {
            self.as_mut().show_password_reset(o.password_reset, &host);
            return;
        }
        self.as_mut().set_reset_note(QString::from(""));
        self.as_mut().set_reset_mode(QString::from("checking"));
        self.as_mut().set_reset_status(QString::from(&crate::password::checking_text(&host)));
        let qt = self.as_mut().qt_thread();
        runtime().spawn(async move {
            let r = crate::password::read_reset(&host).await;
            let _ = qt.queue(move |mut o| {
                if o.rust().reset_check != check {
                    return; // another check, or the face was left, meanwhile
                }
                o.as_mut().show_password_reset(r, &host);
            });
        });
    }

    fn show_password_reset(mut self: Pin<&mut Self>, r: crate::password::PasswordReset, host: &str) {
        let checking = self.reset_mode().to_string() == "checking";
        self.as_mut().set_reset_mode(QString::from(r.name()));
        self.as_mut().set_reset_note(QString::from(&match r {
            crate::password::PasswordReset::Owner => crate::password::owner_reset_text(host),
            crate::password::PasswordReset::Email => String::new(),
        }));
        if checking {
            self.as_mut().set_reset_status(QString::from(""));
        }
    }
}

/// Step 1. Returns (resolved homeserver base url, sid) — the base is carried forward
/// so step 3 cannot end up talking to a different server than step 1 did.
async fn do_request_password_reset(
    server: &str,
    email: &str,
    client_secret: &str,
) -> Result<(String, String), String> {
    let base = resolve_homeserver(server.trim())
        .await?
        .trim_end_matches('/')
        .to_string();
    let url = format!("{base}/_matrix/client/v3/account/password/email/requestToken");
    let body = serde_json::json!({
        "client_secret": client_secret,
        "email": email,
        "send_attempt": 1,
    });
    let resp = matrix_sdk::reqwest::Client::new()
        .post(&url)
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| format!("Could not reach the server: {e}"))?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(matrix_error_message(&text).unwrap_or_else(|| {
            // THREEPID_NOT_FOUND is the common one and its stock wording is unhelpful.
            format!("The server refused the request ({status}).")
        }));
    }
    let sid = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("sid").and_then(|s| s.as_str()).map(str::to_owned))
        .ok_or_else(|| "The server didn't return a session id.".to_string())?;
    Ok((base, sid))
}

/// Step 3. The 401 that carries the UIA session is the expected first response, not
/// an error — only a non-401 failure is.
async fn do_complete_password_reset(
    base: &str,
    client_secret: &str,
    sid: &str,
    new_password: &str,
) -> Result<(), String> {
    let url = format!("{base}/_matrix/client/v3/account/password");
    let http = matrix_sdk::reqwest::Client::new();

    let auth = |session: Option<String>| {
        let mut creds = serde_json::json!({
            "type": "m.login.email.identity",
            "threepid_creds": { "sid": sid, "client_secret": client_secret },
        });
        if let Some(session) = session {
            creds["session"] = serde_json::Value::String(session);
        }
        serde_json::json!({
            "new_password": new_password,
            "logout_devices": true,
            "auth": creds,
        })
    };

    // First attempt without a session: succeeds outright on servers that allow it,
    // otherwise hands us the session to retry with.
    let first = http
        .post(&url)
        .header("Content-Type", "application/json")
        .body(auth(None).to_string())
        .send()
        .await
        .map_err(|e| format!("Could not reach the server: {e}"))?;
    let first_status = first.status();
    let first_text = first.text().await.unwrap_or_default();
    if first_status.is_success() {
        return Ok(());
    }
    let session = serde_json::from_str::<serde_json::Value>(&first_text)
        .ok()
        .and_then(|v| v.get("session").and_then(|s| s.as_str()).map(str::to_owned));
    let Some(session) = session else {
        return Err(matrix_error_message(&first_text).unwrap_or_else(|| {
            format!("The server wouldn't change the password ({first_status}).")
        }));
    };

    let second = http
        .post(&url)
        .header("Content-Type", "application/json")
        .body(auth(Some(session)).to_string())
        .send()
        .await
        .map_err(|e| format!("Could not reach the server: {e}"))?;
    let status = second.status();
    let text = second.text().await.unwrap_or_default();
    if status.is_success() {
        return Ok(());
    }
    Err(matrix_error_message(&text).unwrap_or_else(|| {
        // The overwhelmingly likely cause at this point.
        "Couldn't confirm the reset — click the link in the email first, then try again."
            .to_string()
    }))
}

/// Pull the human-readable `error` out of a Matrix error body, if there is one,
/// and rewrite the two the user is actually likely to hit. Synapse's own wording for
/// both is written for someone reading a spec, not for someone locked out of their
/// account — "Unable to get validated threepid" is what you get for the entirely
/// ordinary mistake of not having opened the email yet.
/// Falls back to None so callers can supply wording that fits their step.
/// 10.1 (TH-FIRST-02 / TH-COPY-07): the sign-in card used to print the raw reqwest / SDK chain
/// ("error sending request for url (…): dns error: …"). One sentence a person can act on; the raw
/// text still goes to the log.
pub(crate) fn friendly_login_error(raw: &str) -> String {
    eprintln!("[login] {raw}");
    let l = raw.to_ascii_lowercase();
    if l.contains("m_forbidden") || l.contains("invalid username") || l.contains("invalid password") || l.contains("forbidden") {
        "Wrong username or password.".to_string()
    } else if l.contains("m_limit_exceeded") || l.contains("429") || l.contains("too many") {
        "Too many attempts — wait a minute and try again.".to_string()
    } else if l.contains("m_user_deactivated") {
        "That account has been deactivated.".to_string()
    } else if l.contains("dns") || l.contains("resolve") || l.contains("no such host") {
        "Can't find that server — check the address.".to_string()
    } else if l.contains("timed out") || l.contains("timeout") || l.contains("connection refused") || l.contains("unreachable") || l.contains("connect") || l.contains("network") {
        "Can't reach the server — check your connection and the address.".to_string()
    } else if l.contains("certificate") || l.contains("tls") || l.contains("ssl") {
        "The server's certificate isn't trusted on this machine.".to_string()
    } else if l.contains("not a valid homeserver") || l.contains("well-known") || l.contains("unsupported") {
        "That doesn't look like a Matrix server.".to_string()
    } else {
        format!("Sign-in failed: {raw}")
    }
}

fn matrix_error_message(body: &str) -> Option<String> {
    let v = serde_json::from_str::<serde_json::Value>(body).ok()?;
    // An unconsumed UIA challenge has no `error`; don't report that as a failure.
    let msg = v.get("error")?.as_str()?.trim();
    if msg.is_empty() {
        return None;
    }
    let errcode = v.get("errcode").and_then(|c| c.as_str()).unwrap_or("");
    Some(match () {
        _ if msg.contains("validated threepid") => {
            "Open the link in the email first — including the confirm button on the page \
             it opens — then set your password here."
                .to_string()
        }
        _ if errcode == "M_THREEPID_NOT_FOUND" => {
            "No account is using that email address.".to_string()
        }
        _ => msg.to_string(),
    })
}


/// Platform-correct initial device name. Was hardcoded to one Linux distribution's name, so
/// every new WINDOWS login showed up as a Linux session in device lists — confusing exactly
/// during the first-run verification flow (a field report).
fn device_display_name() -> &'static str {
    if cfg!(windows) {
        "Tin Hat (Windows)"
    } else {
        "Tin Hat (Linux)"
    }
}
