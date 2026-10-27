//! SettingsController — user account profile (display name + avatar), device/session
//! list, and security/encryption state for the Settings sheet. Mirrors the room.rs /
//! bridge.rs CXX-Qt pattern: async Matrix work on the shared runtime, results posted
//! back to the Qt thread via qt_thread().queue().
use core::pin::Pin;

use cxx_qt::CxxQtType;
use cxx_qt::Threading;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qobject {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, user_id)] //             QML: userId
        #[qproperty(QString, device_id)] //           QML: deviceId — this device
        #[qproperty(QString, display_name)] //        QML: displayName
        #[qproperty(QString, avatar_http)] //         QML: avatarHttp — file:// cached avatar
        #[qproperty(QString, profile_bio)] //         QML: profileBio — short "about me", PUBLIC
        #[qproperty(QString, profile_accent)] //      QML: profileAccent — "#rrggbb" personal colour, PUBLIC
        #[qproperty(QString, join_sound)] //          QML: joinSound — url played when you enter a call
        #[qproperty(QString, card_json)] //           QML: cardJson — ANOTHER user's public profile
        #[qproperty(QString, devices_json)] //        QML: devicesJson — JSON [{id,name,lastSeenTs,lastSeenIp,isCurrent}]
        #[qproperty(QString, bug_report_status)] //   QML: bugReportStatus — in-app report outcome
        #[qproperty(QString, status)] //              QML: status
        #[qproperty(bool, busy)] //                   QML: busy
        #[qproperty(bool, backup_enabled)] //         QML: backupEnabled
        #[qproperty(bool, desktop_notifications)] // QML: desktopNotifications — OS popups for mentions/DMs (default ON)
        #[qproperty(bool, cross_signing_ready)] //    QML: crossSigningReady
        #[qproperty(bool, overlay_enabled)] //        QML: overlayEnabled — voice overlay HUD (default OFF)
        #[qproperty(QString, overlay_mode)] //        QML: overlayMode — "speaking" | "always"
        #[qproperty(QString, overlay_corner)] //      QML: overlayCorner — "tl"|"tr"|"bl"|"br"
        #[qproperty(bool, overlay_hide_share)] //     QML: overlayHideShare — hide HUD while screen-sharing
        #[qproperty(bool, overlay_demo)] //           QML: overlayDemo — TINHAT_OVERLAY_DEMO=1 pins preview pills (benchmarks)
        #[qproperty(QString, call_layout)] //         QML: callLayout — "auto"|"gallery"|"speaker"|"sidebar"
        #[qproperty(QString, call_hidden)] //         QML: callHidden — JSON ["mxid|kind",…]: the hide/show memory (0.8.7)
        #[qproperty(QString, call_tile_view)] //      QML: callTileView — JSON {"mxid|kind":{z,px,py,fill},…} per-person tile view memory (0.8.22)
        #[qproperty(bool, record_optout)] //          QML: recordOptout — "never record me" in Memories call recordings (0.8.7)
        #[qproperty(QString, emails_json)] //         QML: emailsJson — JSON [{address,addedAt}] of bound addresses
        #[qproperty(QString, email_status)] //        QML: emailStatus — human-readable state of the add/remove flow
        #[qproperty(bool, email_pending)] //          QML: emailPending — a link was mailed, waiting for you to click it
        // selfhost: Settings → Devices & security → Password → "Change password" (password.rs)
        #[qproperty(bool, password_busy)] //          QML: passwordBusy — a change is on its way
        #[qproperty(QString, password_outcome)] //    QML: passwordOutcome — the last answer: "changed" | "wrong" | "failed"
        #[qproperty(QString, password_text)] //       QML: passwordText — its words (the toast / the dialog's subtitle)
        #[qproperty(i32, password_answers)] //        QML: passwordAnswers — counts the answers (QML acts on each one once)
        type SettingsController = super::SettingsControllerRust;

        /// QML: settings.refresh() — load profile + devices + security state.
        #[qinvokable]
        fn refresh(self: Pin<&mut SettingsController>);
        /// QML: settings.refreshMyAvatar() — just my display name + photo, no device
        /// enumeration. Cheap enough to run at startup so the footer has a photo before
        /// anyone opens Settings.
        #[qinvokable]
        #[cxx_name = "refreshMyAvatar"]
        fn refresh_my_avatar(self: Pin<&mut SettingsController>);
        /// QML: settings.applyDesktopNotifications(on) — toggle OS notifications for
        /// mentions/DMs. Persisted; takes effect immediately.
        #[qinvokable]
        fn apply_desktop_notifications(self: Pin<&mut SettingsController>, on: bool);
        /// QML: settings.refreshDesktopNotifications() — mirror the persisted toggle.
        #[qinvokable]
        fn refresh_desktop_notifications(self: Pin<&mut SettingsController>);
        /// QML: settings.applyOverlayEnabled(on) — master toggle for the voice overlay HUD.
        #[qinvokable]
        fn apply_overlay_enabled(self: Pin<&mut SettingsController>, on: bool);
        /// QML: settings.applyOverlayMode("speaking"|"always")
        #[qinvokable]
        fn apply_overlay_mode(self: Pin<&mut SettingsController>, mode: &QString);
        /// QML: settings.applyOverlayCorner("tl"|"tr"|"bl"|"br")
        #[qinvokable]
        fn apply_overlay_corner(self: Pin<&mut SettingsController>, corner: &QString);
        /// QML: settings.applyOverlayHideShare(on) — hide the HUD while screen-sharing.
        #[qinvokable]
        fn apply_overlay_hide_share(self: Pin<&mut SettingsController>, on: bool);
        /// QML: settings.applyCallLayout("auto"|"gallery"|"speaker"|"sidebar") — persist
        /// the call-grid preset. Anything else falls back to "auto".
        #[qinvokable]
        fn apply_call_layout(self: Pin<&mut SettingsController>, mode: &QString);
        /// QML: settings.refreshCallLayout() — mirror the persisted preset into callLayout.
        #[qinvokable]
        fn refresh_call_layout(self: Pin<&mut SettingsController>);
        /// 9.3.1: small UI preferences (the call/chat seam share, the collapsed call) — one flag
        /// file per key like the overlay prefs; QML owns the value's shape.
        #[qinvokable]
        fn apply_ui_pref(self: Pin<&mut SettingsController>, key: &QString, value: &QString);
        #[qinvokable]
        fn ui_pref(self: Pin<&mut SettingsController>, key: &QString) -> QString;
        /// 10.1: like the ui prefs but for a JSON blob (drafts, resume positions) — 64 KB cap.
        #[qinvokable]
        fn apply_ui_blob(self: Pin<&mut SettingsController>, key: &QString, value: &QString);
        #[qinvokable]
        fn ui_blob(self: Pin<&mut SettingsController>, key: &QString) -> QString;
        /// QML: settings.applyCallHidden(json) — persist the hide/show memory (JSON
        /// array of "mxid|kind" keys). 0.8.7: hides survive across calls/restarts.
        #[qinvokable]
        fn apply_call_hidden(self: Pin<&mut SettingsController>, json: &QString);
        /// QML: settings.refreshCallHidden() — mirror the persisted hidden-tile keys.
        #[qinvokable]
        fn refresh_call_hidden(self: Pin<&mut SettingsController>);
        /// QML: settings.applyCallTileView(json) — persist the per-person tile view memory.
        #[qinvokable]
        fn apply_call_tile_view(self: Pin<&mut SettingsController>, json: &QString);
        /// QML: settings.applyPref(key, value) / pref(key) — small persisted UI prefs
        /// (soundboard recents, …); key = [a-z0-9-], stored as "ui-<key>".
        #[qinvokable]
        fn apply_pref(self: Pin<&mut SettingsController>, key: &QString, value: &QString);
        #[qinvokable]
        fn pref(self: &SettingsController, key: &QString) -> QString;
        /// QML: settings.refreshCallTileView() — mirror the persisted tile view memory.
        #[qinvokable]
        fn refresh_call_tile_view(self: Pin<&mut SettingsController>);
        /// QML: settings.applyRecordOptout(on) — "never record me": advertised to
        /// peers in our call membership; the Memories recorder skips every track of an
        /// opted-out member and leaves them out of the mixed audio.
        #[qinvokable]
        fn apply_record_optout(self: Pin<&mut SettingsController>, on: bool);
        /// QML: settings.refreshRecordOptout() — mirror the persisted opt-out.
        #[qinvokable]
        fn refresh_record_optout(self: Pin<&mut SettingsController>);
        /// QML: settings.refreshOverlay() — mirror all persisted overlay prefs (+ demo env).
        #[qinvokable]
        fn refresh_overlay(self: Pin<&mut SettingsController>);
        /// QML: settings.submitBugReport(text) — ship the note + the LIVE session log
        /// tail to the problem repo NOW (no restart; the click is the consent).
        /// Outcome lands in bugReportStatus.
        #[qinvokable]
        fn submit_bug_report(self: Pin<&mut SettingsController>, text: &QString);
        /// QML: settings.logshipState() — -1 never asked (show the consent popup),
        /// 0 off, 1 on. See crate::logship.
        #[qinvokable]
        fn logship_state(self: Pin<&mut SettingsController>) -> i32;
        /// QML: settings.applyLogship(on) — persist the debug-log-upload consent.
        #[qinvokable]
        fn apply_logship(self: Pin<&mut SettingsController>, on: bool);
        /// QML: settings.saveDisplayName(name) — (named to avoid colliding with the
        /// auto-generated `set_display_name` setter of the display_name qproperty).
        #[qinvokable]
        fn save_display_name(self: Pin<&mut SettingsController>, name: &QString);
        /// QML: settings.setAvatar(localPathOrFileUrl) — upload + set the account avatar.
        #[qinvokable]
        fn set_avatar(self: Pin<&mut SettingsController>, path: &QString);
        // QML: settingsController.saveProfileBio(text) / saveProfileAccent("#rrggbb").
        // These are EXTENDED PROFILE fields (MSC4133), which this homeserver reports as
        // stable — i.e. other people can actually read them, unlike account data, which
        // is private to you and would make a bio nobody else sees.
        #[qinvokable]
        #[cxx_name = "saveProfileBio"]
        fn save_profile_bio(self: Pin<&mut SettingsController>, bio: &QString);
        #[qinvokable]
        #[cxx_name = "saveProfileAccent"]
        fn save_profile_accent(self: Pin<&mut SettingsController>, hex: &QString);
        // QML: settingsController.saveJoinSound(url) — "" clears it.
        #[qinvokable]
        #[cxx_name = "saveJoinSound"]
        fn save_join_sound(self: Pin<&mut SettingsController>, url: &QString);
        // QML: settingsController.fetchProfileCard(mxid) — populates cardJson with that
        // person's PUBLIC profile fields (bio / accent / join sound).
        #[qinvokable]
        #[cxx_name = "fetchProfileCard"]
        fn fetch_profile_card(self: Pin<&mut SettingsController>, mxid: &QString);
        /// QML: settings.clearAvatar()
        #[qinvokable]
        fn clear_avatar(self: Pin<&mut SettingsController>);
        // ---- Email addresses (3PIDs) ----------------------------------------
        // The account's email is what makes "I forgot my password" recoverable, so
        // the app has to be able to set one; before this you could only do it on the
        // web. Same 3PID on the homeserver either way, so the two clients agree.
        /// QML: settings.refreshEmails() — reload the bound addresses.
        #[qinvokable]
        #[cxx_name = "refreshEmails"]
        fn refresh_emails(self: Pin<&mut SettingsController>);
        /// QML: settings.addEmail(address) — mails a confirmation link.
        #[qinvokable]
        #[cxx_name = "addEmail"]
        fn add_email(self: Pin<&mut SettingsController>, address: &QString);
        /// QML: settings.confirmEmail(password) — call AFTER clicking the emailed
        /// link; the homeserver requires the account password to bind it.
        #[qinvokable]
        #[cxx_name = "confirmEmail"]
        fn confirm_email(self: Pin<&mut SettingsController>, password: &QString);
        /// QML: settings.removeEmail(address)
        #[qinvokable]
        #[cxx_name = "removeEmail"]
        fn remove_email(self: Pin<&mut SettingsController>, address: &QString);
        // ---- Change password (selfhost; password.rs) --------------------------
        /// QML: settings.changePassword(current, newPassword) — the user-interactive two-step POST
        /// /account/password (logout_devices false); the answer lands in passwordOutcome / passwordText and bumps
        /// passwordAnswers. Neither password is logged or kept after the request.
        #[qinvokable]
        #[cxx_name = "changePassword"]
        fn change_password(self: Pin<&mut SettingsController>, current: &QString, new_password: &QString);
        // ---- Settings window search (contract SETTINGS-WINDOW.md §4, settings_search.rs) ----
        /// QML: settings.searchSettings(query, operator) — JSON [{id, pane, paneTitle, section, label, labelHtml,
        /// score}], best first, at most 30; operator-only rows (Invites) only when `is_operator`.
        #[qinvokable]
        #[cxx_name = "searchSettings"]
        fn search_settings(self: &SettingsController, query: &QString, is_operator: bool) -> QString;
        /// QML: settings.settingsPaneOf(id) — the pane key of a search entry ("" when unknown).
        #[qinvokable]
        #[cxx_name = "settingsPaneOf"]
        fn settings_pane_of(self: &SettingsController, id: &QString) -> QString;
    }

    impl cxx_qt::Threading for SettingsController {}
}

#[derive(Default)]
pub struct SettingsControllerRust {
    password_busy: bool,
    password_outcome: QString,
    password_text: QString,
    password_answers: i32,
    emails_json: QString,
    email_status: QString,
    email_pending: bool,
    /// client_secret + sid of an email verification in flight, kept so
    /// confirm_email() can finish the binding the homeserver started.
    pending_email: Option<(String, matrix_sdk::ruma::OwnedSessionId)>,
    desktop_notifications: bool,
    overlay_enabled: bool,
    overlay_mode: QString,
    overlay_corner: QString,
    overlay_hide_share: bool,
    overlay_demo: bool,
    call_layout: QString,
    call_hidden: QString,
    call_tile_view: QString,
    record_optout: bool,
    bug_report_status: QString,
    user_id: QString,
    device_id: QString,
    display_name: QString,
    avatar_http: QString,
    profile_bio: QString,
    profile_accent: QString,
    join_sound: QString,
    card_json: QString,
    devices_json: QString,
    status: QString,
    busy: bool,
    backup_enabled: bool,
    cross_signing_ready: bool,
}

// ---- Voice overlay preferences ------------------------------------------------------
// The overlay itself is pure QML (qml/OverlayWindow.qml, a click-through KWin layer-shell
// surface); only its preferences live in Rust, as flag files in the prefs dir like every other
// persisted toggle (cf. notify.rs, streamer aec.rs).

/// `<config home>/tinhat/<name>` — also the safe-mode start marker (boot.rs). The config
/// home comes from platform::config_home(): Linux/Flatpak = the historical
/// `$XDG_CONFIG_HOME || $HOME/.config`; Windows = the data root's `config\`, never a
/// `.config` relative to the folder the app was started from.
pub(crate) fn overlay_pref_path(name: &str) -> std::path::PathBuf {
    prefs_dir().join(name)
}

/// The prefs dir (`<config>/tinhat`): this device's settings and flag files, and the account's blobs (signout.rs).
pub(crate) fn prefs_dir() -> std::path::PathBuf {
    crate::platform::config_home().join("tinhat")
}

/// The avatar images cached for the account's rooms and people (signout.rs forgets them).
pub(crate) fn avatar_cache_dir() -> std::path::PathBuf {
    crate::platform::cache_base_dir().join("TinHat/avatars")
}

pub(crate) fn overlay_pref_read(name: &str) -> Option<String> {
    std::fs::read_to_string(overlay_pref_path(name)).ok().map(|s| s.trim().to_string())
}

pub(crate) fn overlay_pref_write(name: &str, val: &str) {
    let p = overlay_pref_path(name);
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&p, val);
}

/// Is the voice overlay HUD enabled? Default OFF — it's opt-in, like everything that
/// draws outside the app window.
pub fn overlay_enabled() -> bool {
    overlay_pref_read("overlay-enabled").map(|s| s == "1").unwrap_or(false)
}

/// "Never record me" (default OFF = recordable): read directly where the membership
/// marker is built (rtc.rs) so the advertised flag can never drift from the toggle.
pub fn record_optout() -> bool {
    overlay_pref_read("record-optout").map(|s| s == "1").unwrap_or(false)
}

/// "speaking" (pills appear only while someone talks; surface unmaps when quiet) or
/// "always" (full roster shown for the whole call). Default "speaking".
pub fn overlay_mode() -> String {
    match overlay_pref_read("overlay-mode").as_deref() {
        Some("always") => "always".into(),
        _ => "speaking".into(),
    }
}

/// Screen corner: "tl" | "tr" | "bl" | "br". Default "tl".
pub fn overlay_corner() -> String {
    match overlay_pref_read("overlay-corner").as_deref() {
        Some(c @ ("tr" | "bl" | "br")) => c.into(),
        _ => "tl".into(),
    }
}

/// Hide the HUD while screen-sharing (it is composited into a monitor capture, so
/// friends would see it in the share). Default OFF — it only shows the call roster,
/// which they can see anyway.
pub fn overlay_hide_share() -> bool {
    overlay_pref_read("overlay-hide-share").map(|s| s == "1").unwrap_or(false)
}

/// One-line summary for the diagnostics report.
pub fn overlay_summary() -> String {
    if overlay_enabled() {
        format!("on ({}, corner {})", overlay_mode(), overlay_corner())
    } else {
        "off".into()
    }
}

impl qobject::SettingsController {
    /// Toggle OS desktop notifications for mentions/DMs (persisted; immediate).
    pub fn apply_desktop_notifications(mut self: Pin<&mut Self>, on: bool) {
        crate::notify::set_enabled(on);
        self.as_mut().set_desktop_notifications(on);
    }

    pub fn apply_overlay_enabled(mut self: Pin<&mut Self>, on: bool) {
        overlay_pref_write("overlay-enabled", if on { "1" } else { "0" });
        self.as_mut().set_overlay_enabled(on);
    }

    pub fn apply_overlay_mode(mut self: Pin<&mut Self>, mode: &QString) {
        let m = if mode.to_string() == "always" { "always" } else { "speaking" };
        overlay_pref_write("overlay-mode", m);
        self.as_mut().set_overlay_mode(QString::from(m));
    }

    pub fn apply_overlay_corner(mut self: Pin<&mut Self>, corner: &QString) {
        let c = match corner.to_string().as_str() {
            c @ ("tr" | "bl" | "br") => c.to_string(),
            _ => "tl".to_string(),
        };
        overlay_pref_write("overlay-corner", &c);
        self.as_mut().set_overlay_corner(QString::from(&c));
    }

    /// The call-grid preset, one small file like the overlay prefs.
    pub fn apply_call_layout(mut self: Pin<&mut Self>, mode: &QString) {
        let m = match mode.to_string().as_str() {
            m @ ("gallery" | "speaker" | "sidebar") => m.to_string(),
            _ => "auto".to_string(),
        };
        overlay_pref_write("call-layout", &m);
        self.as_mut().set_call_layout(QString::from(&m));
    }

    /// 9.3.1: generic small UI pref. Keys are restricted to `[a-z0-9-]` so a QML typo can
    /// never write outside the pref directory; values are capped at 256 bytes.
    pub fn apply_ui_pref(self: Pin<&mut Self>, key: &QString, value: &QString) {
        let key = key.to_string();
        if key.is_empty() || key.len() > 40 || !key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return;
        }
        let mut v = value.to_string();
        v.truncate(256);
        overlay_pref_write(&format!("ui-{key}"), &v);
    }

    pub fn ui_pref(self: Pin<&mut Self>, key: &QString) -> QString {
        let key = key.to_string();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return QString::default();
        }
        QString::from(&overlay_pref_read(&format!("ui-{key}")).unwrap_or_default())
    }

    /// 10.1: a JSON blob under the same key rules, capped at 64 KB (chat drafts, Memories resume
    /// positions). Stored as `blob-<key>` beside the ui prefs.
    pub fn apply_ui_blob(self: Pin<&mut Self>, key: &QString, value: &QString) {
        let key = key.to_string();
        if key.is_empty() || key.len() > 40 || !key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return;
        }
        let v = value.to_string();
        if v.len() > 65_536 {
            return;
        }
        overlay_pref_write(&format!("blob-{key}"), &v);
    }

    pub fn ui_blob(self: Pin<&mut Self>, key: &QString) -> QString {
        let key = key.to_string();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return QString::default();
        }
        QString::from(&overlay_pref_read(&format!("blob-{key}")).unwrap_or_default())
    }

    pub fn refresh_call_layout(mut self: Pin<&mut Self>) {
        let m = match overlay_pref_read("call-layout").as_deref() {
            Some(m @ ("gallery" | "speaker" | "sidebar")) => m.to_string(),
            _ => "auto".to_string(),
        };
        self.as_mut().set_call_layout(QString::from(&m));
    }

    /// 0.8.7 hide/show memory: which call tiles ("mxid|kind") stay hidden across
    /// calls. Stored as the QML-produced JSON array in one flag file like the other
    /// prefs — the engine only round-trips it, QML owns the shape.
    pub fn apply_call_hidden(mut self: Pin<&mut Self>, json: &QString) {
        let j = json.to_string();
        // Only ever store what parses as a string array — a corrupt write must not
        // be able to wedge the startup restore (QML's JSON.parse would throw).
        let j = if serde_json::from_str::<Vec<String>>(&j).is_ok() { j } else { "[]".to_string() };
        overlay_pref_write("call-hidden", &j);
        self.as_mut().set_call_hidden(QString::from(&j));
    }

    pub fn apply_pref(self: Pin<&mut Self>, key: &QString, value: &QString) {
        let k: String = key.to_string().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(40).collect();
        if k.is_empty() {
            return;
        }
        overlay_pref_write(&format!("ui-{k}"), &value.to_string());
    }

    pub fn pref(&self, key: &QString) -> QString {
        let k: String = key.to_string().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(40).collect();
        QString::from(&overlay_pref_read(&format!("ui-{k}")).unwrap_or_default())
    }

    /// Settings window search (SETTINGS-WINDOW.md §4): the shared matcher over the embedded index.
    pub fn search_settings(&self, query: &QString, is_operator: bool) -> QString {
        QString::from(&crate::settings_search::search_json(&query.to_string(), is_operator))
    }

    pub fn settings_pane_of(&self, id: &QString) -> QString {
        QString::from(crate::settings_search::pane_of(&id.to_string()))
    }

    pub fn apply_call_tile_view(mut self: Pin<&mut Self>, json: &QString) {
        let j = json.to_string();
        let j = if serde_json::from_str::<serde_json::Value>(&j).map(|v| v.is_object()).unwrap_or(false) { j } else { "{}".to_string() };
        overlay_pref_write("call-tileview", &j);
        self.as_mut().set_call_tile_view(QString::from(&j));
    }

    pub fn refresh_call_tile_view(mut self: Pin<&mut Self>) {
        let j = overlay_pref_read("call-tileview")
            .filter(|s| serde_json::from_str::<serde_json::Value>(s).map(|v| v.is_object()).unwrap_or(false))
            .unwrap_or_else(|| "{}".to_string());
        self.as_mut().set_call_tile_view(QString::from(&j));
    }

    pub fn refresh_call_hidden(mut self: Pin<&mut Self>) {
        let j = overlay_pref_read("call-hidden")
            .filter(|s| serde_json::from_str::<Vec<String>>(s).is_ok())
            .unwrap_or_else(|| "[]".to_string());
        self.as_mut().set_call_hidden(QString::from(&j));
    }

    /// 0.8.7 "never record me": the Memories recorder must not capture this user's
    /// audio/video. Persisted here, advertised via rtc.rs's membership marker.
    pub fn apply_record_optout(mut self: Pin<&mut Self>, on: bool) {
        overlay_pref_write("record-optout", if on { "1" } else { "0" });
        self.as_mut().set_record_optout(on);
    }

    pub fn refresh_record_optout(mut self: Pin<&mut Self>) {
        let on = record_optout();
        self.as_mut().set_record_optout(on);
    }

    pub fn apply_overlay_hide_share(mut self: Pin<&mut Self>, on: bool) {
        overlay_pref_write("overlay-hide-share", if on { "1" } else { "0" });
        self.as_mut().set_overlay_hide_share(on);
    }

    /// In-app bug report (room-list 🐞): ship note + live-log tail immediately.
    pub fn submit_bug_report(mut self: Pin<&mut Self>, text: &QString) {
        let note = text.to_string();
        self.as_mut().set_bug_report_status(QString::from("Sending report…"));
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let user = crate::session::client()
                .and_then(|c| c.user_id().map(|u| u.to_string()))
                .unwrap_or_else(|| "@unknown".into());
            let outcome = crate::logship::ship_bug_report(&user, &note).await;
            let server = user.split_once(':').map(|(_, s)| s.to_string()).unwrap_or_default();
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_bug_report_status(QString::from(match &outcome {
                    // Give the user their ticket number: a report you can follow is the
                    // difference between reporting a bug and shouting into a log file.
                    // (the web's self-hosted words, settings/logs.ts: "… tracked as <id> on <server>.")
                    Ok(id) if !id.is_empty() => format!("Report sent — tracked as {id} on {server}."),
                    Ok(_) => "Report sent — thank you! It landed in the problem repo.".to_string(),
                    Err(e) => format!("Report failed: {e}"),
                }.as_str()));
            });
        });
    }

    pub fn logship_state(self: Pin<&mut Self>) -> i32 {
        crate::logship::consent_state()
    }

    pub fn apply_logship(self: Pin<&mut Self>, on: bool) {
        crate::logship::set_consent(on);
    }

    pub fn refresh_overlay(mut self: Pin<&mut Self>) {
        self.as_mut().set_overlay_enabled(overlay_enabled());
        self.as_mut().set_overlay_mode(QString::from(&overlay_mode()));
        self.as_mut().set_overlay_corner(QString::from(&overlay_corner()));
        self.as_mut().set_overlay_hide_share(overlay_hide_share());
        let demo = std::env::var("TINHAT_OVERLAY_DEMO").ok().as_deref() == Some("1");
        self.as_mut().set_overlay_demo(demo);
    }

    /// Mirror the persisted notification toggle into `desktopNotifications` for QML.
    pub fn refresh_desktop_notifications(mut self: Pin<&mut Self>) {
        let on = crate::notify::enabled();
        self.as_mut().set_desktop_notifications(on);
    }

    /// Display name + avatar only. `refresh()` also lists every device, which is a
    /// slow call on a long-lived account and pointless when all we want is the photo.
    pub fn refresh_my_avatar(mut self: Pin<&mut Self>) {
        let Some(client) = crate::session::client() else { return };
        if let Some(uid) = client.user_id() {
            self.as_mut().set_user_id(QString::from(&uid.to_string()));
        }
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let account = client.account();
            let name = account.get_display_name().await.ok().flatten().unwrap_or_default();
            let http = match account.get_avatar_url().await.ok().flatten() {
                Some(mxc) => resolve_avatar(&client, &mxc).await.unwrap_or_default(),
                None => String::new(),
            };
            eprintln!("[avatar] mine resolved to {:?}", http);
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_display_name(QString::from(&name));
                o.as_mut().set_avatar_http(QString::from(&http));
            });
        });
    }

    pub fn refresh(mut self: Pin<&mut Self>) {
        let Some(client) = crate::session::client() else {
            return;
        };
        self.as_mut().set_busy(true);
        self.as_mut().set_status(QString::default());
        if let Some(uid) = client.user_id() {
            self.as_mut().set_user_id(QString::from(&uid.to_string()));
        }
        if let Some(did) = client.device_id() {
            self.as_mut().set_device_id(QString::from(&did.to_string()));
        }
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let account = client.account();
            let name = account.get_display_name().await.ok().flatten().unwrap_or_default();
            let avatar_http = match account.get_avatar_url().await.ok().flatten() {
                Some(mxc) => resolve_avatar(&client, &mxc).await.unwrap_or_default(),
                None => String::new(),
            };
            let cur = client.device_id().map(|d| d.to_string()).unwrap_or_default();
            let devices_json = match client.devices().await {
                Ok(resp) => {
                    let arr: Vec<serde_json::Value> = resp
                        .devices
                        .iter()
                        .map(|d| {
                            serde_json::json!({
                                "id": d.device_id.to_string(),
                                "name": d.display_name.clone().unwrap_or_default(),
                                "lastSeenTs": d.last_seen_ts.map(|t| u64::from(t.0)).unwrap_or(0),
                                "lastSeenIp": d.last_seen_ip.clone().unwrap_or_default(),
                                "isCurrent": d.device_id.as_str() == cur,
                            })
                        })
                        .collect();
                    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".into())
                }
                Err(e) => {
                    eprintln!("[settings] devices failed: {e}");
                    "[]".into()
                }
            };
            let (profile_bio, profile_accent, join_sound) = match client.user_id() {
                Some(uid) => (
                    get_profile_field(&client, uid, BIO_KEY).await.unwrap_or_default(),
                    get_profile_field(&client, uid, ACCENT_KEY).await.unwrap_or_default(),
                    get_profile_field(&client, uid, crate::joinsound::JOINSOUND_KEY)
                        .await
                        .unwrap_or_default(),
                ),
                None => (String::new(), String::new(), String::new()),
            };
            let backup_enabled = client.encryption().backups().are_enabled().await;
            let cross_signing_ready = client
                .encryption()
                .cross_signing_status()
                .await
                .map(|s| s.has_master && s.has_self_signing && s.has_user_signing)
                .unwrap_or(false);
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_display_name(QString::from(&name));
                o.as_mut().set_avatar_http(QString::from(&avatar_http));
                o.as_mut().set_profile_bio(QString::from(&profile_bio));
                o.as_mut().set_profile_accent(QString::from(&profile_accent));
                o.as_mut().set_join_sound(QString::from(&join_sound));
                o.as_mut().set_devices_json(QString::from(&devices_json));
                o.as_mut().set_backup_enabled(backup_enabled);
                o.as_mut().set_cross_signing_ready(cross_signing_ready);
                o.as_mut().set_busy(false);
            });
        });
    }

    pub fn save_display_name(mut self: Pin<&mut Self>, name: &QString) {
        let Some(client) = crate::session::client() else {
            return;
        };
        let name = name.to_string();
        self.as_mut().set_busy(true);
        self.as_mut().set_status(QString::from("Saving…"));
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let r = client.account().set_display_name(Some(&name)).await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_busy(false);
                match r {
                    Ok(()) => {
                        o.as_mut().set_display_name(QString::from(&name));
                        o.as_mut().set_status(QString::from("Saved"));
                    }
                    Err(e) => o.as_mut().set_status(QString::from(&format!("Couldn't save: {e}"))),
                }
            });
        });
    }

    /// Save the public "about me" line.
    pub fn save_profile_bio(mut self: Pin<&mut Self>, bio: &QString) {
        let bio = bio.to_string();
        self.as_mut().set_profile_bio(QString::from(&bio));
        self.save_profile_field(BIO_KEY, bio);
    }

    /// Save the personal accent colour ("#rrggbb"). Rejected rather than stored if it
    /// isn't a hex colour: this value is rendered straight into other people's UI, so
    /// letting arbitrary text through would be someone else's rendering bug.
    pub fn save_profile_accent(mut self: Pin<&mut Self>, hex: &QString) {
        let hex = hex.to_string();
        let ok = hex.is_empty()
            || (hex.len() == 7
                && hex.starts_with('#')
                && hex[1..].chars().all(|c| c.is_ascii_hexdigit()));
        if !ok {
            self.as_mut()
                .set_status(QString::from("That isn't a colour like #7aa2f7"));
            return;
        }
        self.as_mut().set_profile_accent(QString::from(&hex));
        self.save_profile_field(ACCENT_KEY, hex);
    }

    /// Choose (or clear, with "") the sound published when you walk into a call.
    pub fn save_join_sound(mut self: Pin<&mut Self>, url: &QString) {
        let url = url.to_string();
        self.as_mut().set_join_sound(QString::from(&url));
        self.save_profile_field(crate::joinsound::JOINSOUND_KEY, url);
    }

    /// Load someone else's public profile fields for the profile card. Clears the
    /// previous card first so a slow fetch can never show the last person's bio under
    /// the new person's name.
    pub fn fetch_profile_card(mut self: Pin<&mut Self>, mxid: &QString) {
        let Some(client) = crate::session::client() else { return };
        let mxid = mxid.to_string();
        self.as_mut().set_card_json(QString::from(
            &serde_json::json!({ "mxid": mxid, "loading": true }).to_string(),
        ));
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let Ok(uid) = matrix_sdk::ruma::UserId::parse(mxid.as_str()) else { return };
            let bio = get_profile_field(&client, &uid, BIO_KEY).await.unwrap_or_default();
            let accent = get_profile_field(&client, &uid, ACCENT_KEY).await.unwrap_or_default();
            let join_sound = get_profile_field(&client, &uid, crate::joinsound::JOINSOUND_KEY)
                .await
                .unwrap_or_default();
            let json = serde_json::json!({
                "mxid": mxid,
                "bio": bio,
                "accent": accent,
                "joinSound": join_sound,
                "loading": false,
            })
            .to_string();
            let _ = qt.queue(move |mut o| o.as_mut().set_card_json(QString::from(&json)));
        });
    }

    /// Shared writer for the extended-profile fields.
    fn save_profile_field(mut self: Pin<&mut Self>, key: &'static str, value: String) {
        let Some(client) = crate::session::client() else { return };
        self.as_mut().set_busy(true);
        self.as_mut().set_status(QString::from("Saving…"));
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let r = set_profile_field(&client, key, &value).await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_busy(false);
                match r {
                    Ok(()) => o.as_mut().set_status(QString::from("Saved")),
                    Err(e) => {
                        o.as_mut().set_status(QString::from(&format!("Couldn't save: {e}")))
                    }
                }
            });
        });
    }

    pub fn set_avatar(mut self: Pin<&mut Self>, path: &QString) {
        let Some(client) = crate::session::client() else {
            return;
        };
        let path = path.to_string();
        self.as_mut().set_busy(true);
        self.as_mut().set_status(QString::from("Uploading avatar…"));
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let r = upload_and_set_avatar(&client, &path).await;
            let http = if r.is_ok() {
                match client.account().get_avatar_url().await.ok().flatten() {
                    Some(mxc) => resolve_avatar(&client, &mxc).await.unwrap_or_default(),
                    None => String::new(),
                }
            } else {
                String::new()
            };
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_busy(false);
                match r {
                    Ok(()) => {
                        if !http.is_empty() {
                            o.as_mut().set_avatar_http(QString::from(&http));
                        }
                        o.as_mut().set_status(QString::from("Avatar updated"));
                    }
                    Err(e) => o.as_mut().set_status(QString::from(&format!("Couldn't upload: {e}"))),
                }
            });
        });
    }

    pub fn clear_avatar(mut self: Pin<&mut Self>) {
        let Some(client) = crate::session::client() else {
            return;
        };
        self.as_mut().set_busy(true);
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let r = client.account().set_avatar_url(None).await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_busy(false);
                if r.is_ok() {
                    o.as_mut().set_avatar_http(QString::default());
                    o.as_mut().set_status(QString::from("Avatar removed"));
                }
            });
        });
    }
}

/// Password login over raw HTTP, returning an access token — for gates that need a
/// SECOND identity in the same process (see headless_profile_test).
async fn raw_login(hs: &str, user: &str, pass: &str) -> Option<String> {
    let body = serde_json::json!({
        "type": "m.login.password",
        "identifier": { "type": "m.id.user", "user": user },
        "password": pass,
    })
    .to_string();
    let text = matrix_sdk::reqwest::Client::new()
        .post(format!("{}/_matrix/client/v3/login", hs.trim_end_matches('/')))
        .header("Content-Type", "application/json")
        .body(body)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()?;
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()?
        .get("access_token")
        .and_then(|t| t.as_str())
        .map(str::to_owned)
}

/// Read one extended-profile field as an arbitrary token holder.
async fn raw_profile_field(hs: &str, token: &str, user: &str, key: &str) -> Option<String> {
    let text = matrix_sdk::reqwest::Client::new()
        .get(format!(
            "{}/_matrix/client/v3/profile/{}/{}",
            hs.trim_end_matches('/'),
            user,
            key
        ))
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()?;
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()?
        .get(key)
        .and_then(|x| x.as_str())
        .map(str::to_owned)
}

/// Headless gate for QOL 1.5.1 — proves profile fields are PUBLIC, which is the only
/// reason to use extended profiles over account data:
///   TINHAT_PROFILE_TEST=1 TINHAT_USER=… TINHAT_PASS=… [TINHAT_USER2=… TINHAT_PASS2=…] ./tinhat
///
/// Writes a bio + accent as one account, reads them back as that account, and — when a
/// second account is supplied — reads them again as SOMEONE ELSE. A bio only that
/// author can see is the failure this catches. Exits 0/1.
pub async fn headless_profile_test() -> bool {
    let mut fails = 0;
    let mut check = |label: &str, ok: bool| {
        eprintln!("[profile] {} {label}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            fails += 1;
        }
    };
    let (Ok(user), Ok(pass)) = (std::env::var("TINHAT_USER"), std::env::var("TINHAT_PASS")) else {
        eprintln!("[profile] FAIL: need TINHAT_USER + TINHAT_PASS");
        return false;
    };
    let hs = match crate::bridge::env_homeserver() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("[profile] FAIL: {e}");
            return false;
        }
    };
    let client = match crate::bridge::headless_login(&hs, &user, &pass).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[profile] FAIL: login: {e}");
            return false;
        }
    };
    let me = client.user_id().map(|u| u.to_owned()).unwrap();

    let bio = format!("gate bio {}", me.localpart());
    let accent = "#7aa2f7";
    check(
        "bio written",
        set_profile_field(&client, BIO_KEY, &bio).await.is_ok(),
    );
    check(
        "accent written",
        set_profile_field(&client, ACCENT_KEY, accent).await.is_ok(),
    );
    check(
        "bio reads back for the author",
        get_profile_field(&client, &me, BIO_KEY).await.as_deref() == Some(bio.as_str()),
    );
    check(
        "accent reads back for the author",
        get_profile_field(&client, &me, ACCENT_KEY).await.as_deref() == Some(accent),
    );

    // The point of the whole design: someone ELSE can see it.
    // The second account logs in over RAW HTTP rather than through the SDK: two SDK
    // clients in one process share one sqlite crypto store, and on Windows the first
    // client still holds those files open so the store cannot be wiped for the second
    // ("the account in the store doesn't match the account in the constructor").
    // Visibility is an HTTP-level property anyway, so this tests it directly.
    match (std::env::var("TINHAT_USER2"), std::env::var("TINHAT_PASS2")) {
        (Ok(u2), Ok(p2)) => match raw_login(&hs, &u2, &p2).await {
            Some(token) => {
                check(
                    "another user can read the bio (it is public)",
                    raw_profile_field(&hs, &token, me.as_str(), BIO_KEY).await.as_deref()
                        == Some(bio.as_str()),
                );
                check(
                    "another user can read the accent",
                    raw_profile_field(&hs, &token, me.as_str(), ACCENT_KEY).await.as_deref()
                        == Some(accent),
                );
            }
            None => check("second account login", false),
        },
        _ => eprintln!("[profile] (skipped public-read check: no TINHAT_USER2/TINHAT_PASS2)"),
    }

    // An unset field must read as None, not as a stale value or an error string.
    check(
        "unknown field reads as unset",
        get_profile_field(&client, &me, "dev.tinhat.doesnotexist").await.is_none(),
    );

    eprintln!("[profile] VERDICT: {}", if fails == 0 { "PASS" } else { "FAIL" });
    fails == 0
}

/// Extended-profile keys (MSC4133). Namespaced so they can never collide with a
/// future spec field.
pub(crate) const BIO_KEY: &str = "dev.tinhat.bio";
pub(crate) const ACCENT_KEY: &str = "dev.tinhat.accent";

/// Read one extended-profile field for any user (public — this is what makes a bio
/// worth having). `None` when unset: the server 404s the key rather than returning null.
pub(crate) async fn get_profile_field(
    client: &matrix_sdk::Client,
    user: &matrix_sdk::ruma::UserId,
    key: &str,
) -> Option<String> {
    let url = format!(
        "{}_matrix/client/v3/profile/{}/{}",
        client.homeserver(),
        user,
        key
    );
    let token = client.access_token()?;
    let resp = matrix_sdk::reqwest::Client::new()
        .get(&url)
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let text = resp.text().await.ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get(key).and_then(|x| x.as_str()).map(str::to_owned)
}

/// Write one extended-profile field for OURSELVES.
pub(crate) async fn set_profile_field(
    client: &matrix_sdk::Client,
    key: &str,
    value: &str,
) -> Result<(), String> {
    let user = client.user_id().ok_or_else(|| "Not signed in".to_string())?;
    let token = client.access_token().ok_or_else(|| "Not signed in".to_string())?;
    let url = format!(
        "{}_matrix/client/v3/profile/{}/{}",
        client.homeserver(),
        user,
        key
    );
    let body = serde_json::json!({ key: value }).to_string();
    let resp = matrix_sdk::reqwest::Client::new()
        .put(&url)
        .bearer_auth(token)
        .header("Content-Type", "application/json")
        .body(body)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("server said HTTP {}", resp.status()))
    }
}

async fn upload_and_set_avatar(client: &matrix_sdk::Client, path_or_url: &str) -> Result<(), String> {
    let path: std::path::PathBuf = if path_or_url.starts_with("file://") {
        url::Url::parse(path_or_url)
            .ok()
            .and_then(|u| u.to_file_path().ok())
            .ok_or_else(|| format!("bad file url: {path_or_url}"))?
    } else {
        std::path::PathBuf::from(path_or_url)
    };
    let data = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let mime: mime::Mime = avatar_mime(&ext).parse().unwrap_or(mime::IMAGE_PNG);
    let mxc = client
        .account()
        .upload_avatar(&mime, data)
        .await
        .map_err(|e| e.to_string())?;
    client
        .account()
        .set_avatar_url(Some(&mxc))
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn avatar_mime(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "image/png",
    }
}

/// Resolve an avatar mxc to a cached local `file://` (best-effort, Path::exists short-circuit).
async fn resolve_avatar(client: &matrix_sdk::Client, mxc: &matrix_sdk::ruma::MxcUri) -> Option<String> {
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
    let dir = avatar_cache_dir();
    // security review 2026-10-26: only a raster image's bytes, cached as that format (viewer.rs) — never SVG
    if let Some(path) = crate::viewer::cached_image(&dir, &id) {
        return Some(crate::platform::file_url(&path));
    }
    let req = MediaRequestParameters {
        source: MediaSource::Plain(mxc.to_owned()),
        format: MediaFormat::File,
    };
    let bytes = client.media().get_media_content(&req, true).await.ok()?;
    let path = crate::viewer::image_path_for(&dir, &id, &bytes)?;
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(&path, &bytes).ok()?;
    Some(crate::platform::file_url(&path))
}

// ---------------------------------------------------------------------------
// Email addresses (3PIDs)
//
// Flow, as the homeserver defines it: request a token (which mails a link), the
// user clicks the link, then the client calls add_3pid with the account password
// as user-interactive auth. The sid from step one has to survive until step
// three, so it is held on the controller rather than passed through QML.
// ---------------------------------------------------------------------------
impl qobject::SettingsController {
    pub fn refresh_emails(mut self: Pin<&mut Self>) {
        let Some(client) = crate::session::client() else { return };
        let qt = self.as_mut().qt_thread();
        crate::bridge::runtime().spawn(async move {
            let r = client.account().get_3pids().await;
            let json = match r {
                Ok(resp) => {
                    let items: Vec<String> = resp
                        .threepids
                        .iter()
                        .filter(|t| t.medium == matrix_sdk::ruma::thirdparty::Medium::Email)
                        .map(|t| {
                            format!(
                                r#"{{"address":{}}}"#,
                                serde_json::to_string(&t.address)
                                    .unwrap_or_else(|_| "\"\"".into())
                            )
                        })
                        .collect();
                    format!("[{}]", items.join(","))
                }
                Err(_) => "[]".to_string(),
            };
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_emails_json(QString::from(&json));
            });
        });
    }

    pub fn add_email(mut self: Pin<&mut Self>, address: &QString) {
        let Some(client) = crate::session::client() else { return };
        let address = address.to_string();
        if address.trim().is_empty() {
            self.as_mut().set_email_status(QString::from("Enter an email address."));
            return;
        }
        // A fresh secret per attempt: reusing one across addresses would let the
        // homeserver tie the second request to the first address.
        let secret = format!(
            "tinhat{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        self.as_mut().set_busy(true);
        self.as_mut().set_email_status(QString::from("Sending confirmation email…"));
        let qt = self.as_mut().qt_thread();
        let secret_for_task = secret.clone();
        crate::bridge::runtime().spawn(async move {
            let r = client
                .account()
                .request_3pid_email_token(
                    <&matrix_sdk::ruma::ClientSecret>::try_from(secret_for_task.as_str())
                        .expect("generated client secret is valid"),
                    &address,
                    uint(1),
                )
                .await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_busy(false);
                match r {
                    Ok(resp) => {
                        o.as_mut().rust_mut().pending_email = Some((secret_for_task, resp.sid));
                        o.as_mut().set_email_pending(true);
                        o.as_mut().set_email_status(QString::from(&format!(
                            "Check {address} and click the link, then enter your password below."
                        )));
                    }
                    Err(e) => {
                        o.as_mut().set_email_pending(false);
                        o.as_mut()
                            .set_email_status(QString::from(&format!("Couldn't send the email: {e}")));
                    }
                }
            });
        });
    }

    pub fn confirm_email(mut self: Pin<&mut Self>, password: &QString) {
        let Some(client) = crate::session::client() else { return };
        let Some((secret, sid)) = self.as_mut().rust_mut().pending_email.clone() else {
            self.as_mut()
                .set_email_status(QString::from("Add an address first."));
            return;
        };
        let password = password.to_string();
        if password.is_empty() {
            self.as_mut()
                .set_email_status(QString::from("Enter your account password to confirm."));
            return;
        }
        let user_id = client.user_id().map(|u| u.to_string()).unwrap_or_default();
        self.as_mut().set_busy(true);
        self.as_mut().set_email_status(QString::from("Confirming…"));
        let qt = self.as_mut().qt_thread();
        crate::bridge::runtime().spawn(async move {
            use matrix_sdk::ruma::api::client::uiaa;
            // First call comes back 401 with a session id; the second carries the
            // password. Doing it in one shot is not possible — the session is minted
            // by the rejection.
            let secret_typed = <&matrix_sdk::ruma::ClientSecret>::try_from(secret.as_str())
                .expect("stored client secret is valid");
            let first = client.account().add_3pid(secret_typed, &sid, None).await;
            let session = match &first {
                Err(e) => e
                    .as_uiaa_response()
                    .and_then(|info| info.session.clone()),
                Ok(_) => None,
            };
            let r = if first.is_ok() {
                Ok(())
            } else if let Some(session) = session {
                let mut pw = uiaa::Password::new(
                    uiaa::UserIdentifier::Matrix(uiaa::MatrixUserIdentifier::new(user_id)),
                    password,
                );
                pw.session = Some(session);
                client
                    .account()
                    .add_3pid(secret_typed, &sid, Some(uiaa::AuthData::Password(pw)))
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            } else {
                first.map(|_| ()).map_err(|e| e.to_string())
            };
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_busy(false);
                match r {
                    Ok(()) => {
                        o.as_mut().rust_mut().pending_email = None;
                        o.as_mut().set_email_pending(false);
                        o.as_mut().set_email_status(QString::from("Email added."));
                        o.as_mut().refresh_emails();
                    }
                    Err(e) => {
                        o.as_mut()
                            .set_email_status(QString::from(&format!("Couldn't confirm: {e}")));
                    }
                }
            });
        });
    }

    pub fn remove_email(mut self: Pin<&mut Self>, address: &QString) {
        let Some(client) = crate::session::client() else { return };
        let address = address.to_string();
        self.as_mut().set_busy(true);
        self.as_mut().set_email_status(QString::from("Removing…"));
        let qt = self.as_mut().qt_thread();
        crate::bridge::runtime().spawn(async move {
            let r = client
                .account()
                .delete_3pid(&address, matrix_sdk::ruma::thirdparty::Medium::Email, None)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string());
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_busy(false);
                match r {
                    Ok(()) => {
                        o.as_mut().set_email_status(QString::from("Email removed."));
                        o.as_mut().refresh_emails();
                    }
                    Err(e) => {
                        o.as_mut()
                            .set_email_status(QString::from(&format!("Couldn't remove: {e}")));
                    }
                }
            });
        });
    }
}

// ---------------------------------------------------------------------------
// Change password (selfhost, 2026-10-22: a self-hosted server sends no email, so the owner hands out a new
// password and you choose your own here). Standard Matrix user-interactive auth, by hand (password.rs): the SDK's
// change_password always signs the other devices out, and both apps send exactly the same two bodies.
// ---------------------------------------------------------------------------
impl qobject::SettingsController {
    pub fn change_password(mut self: Pin<&mut Self>, current: &QString, new_password: &QString) {
        if *self.password_busy() {
            return;
        }
        let Some(client) = crate::session::client() else { return };
        let Some(user) = client.user_id().map(|u| u.to_string()) else { return };
        // a temporary guest account has no password of its own to change (the row is hidden for it)
        if crate::server_offer::is_guest_account(&user) {
            return;
        }
        let Some(token) = client.access_token() else { return };
        let current = current.to_string();
        let new_password = new_password.to_string();
        if !crate::password::change_ready(&current, &new_password, &new_password) {
            return;
        }
        let homeserver = client.homeserver().to_string();
        self.as_mut().set_password_busy(true);
        let qt = self.as_mut().qt_thread();
        crate::bridge::runtime().spawn(async move {
            let out = crate::password::change_password(&homeserver, &token, &user, &current, &new_password).await;
            drop(current);
            drop(new_password);
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_password_busy(false);
                o.as_mut().set_password_outcome(QString::from(out.kind()));
                o.as_mut().set_password_text(QString::from(&out.text()));
                let n = *o.password_answers();
                o.as_mut().set_password_answers(n.wrapping_add(1));
            });
        });
    }
}

fn uint(n: u64) -> matrix_sdk::ruma::UInt {
    matrix_sdk::ruma::UInt::try_from(n).unwrap_or_default()
}
