//! What signing out forgets on this device besides the Matrix stores and the saved session (bridge.rs
//! finish_sign_out): everything the app kept of the ACCOUNT, as Tin Hat Web's sign-out does (session.ts doLogout: its
//! stores and the account's keys `tinhat.blob.*`, `tinhat.user.*`, `tinhat.cache.*`, last-room, soundboard-recent).
//!
//! * On disk: the decrypted timeline snapshots, member lists, decrypted media, link previews and avatars the rooms
//!   cache, the Memories thumbnails, the mentions list, the dice rolls, the widgets' history, the clocks' last write,
//!   the JSON blobs (chat drafts, Memories resume points), the last room, the soundboard's recent sounds.
//! * In memory: the rooms' windows (decrypted events), local echoes, pinned rows, the mentions, the dice, the widgets'
//!   engine, the link previews.
//!
//! This device's settings stay (the other prefs and flag files: theme, sounds, window, call devices; the tour; the
//! theme offers; a pending presence "off" the account's next start finishes). Found by the move's desktop test
//! (2026-10-20): the next sign-in was shown the previous session's decrypted messages before it could decrypt any.

/// The account's files among the prefs dir's entries (`<config>/tinhat`), by name: the JSON blobs (chat drafts,
/// Memories resume points), the last room, the soundboard's recent sounds (the web's `tinhat.blob.*`,
/// `tinhat.pref.last-room`, `tinhat.pref.soundboard-recent`). The rest are this device's settings.
pub(crate) fn account_pref_files<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    names
        .into_iter()
        .filter(|n| n.starts_with("blob-") || *n == "ui-last-room" || *n == "ui-soundboard-recent")
        .collect()
}

/// Forget the account (on the GUI thread, after the client is gone: nothing writes these back for it).
pub(crate) fn forget_account() {
    for dir in [
        crate::room::timeline_cache_dir(),
        crate::room::members_cache_dir(),
        crate::room::media_cache_dir(),
        crate::linkpreview::cache_dir(),
        crate::settings::avatar_cache_dir(),
        crate::bridge::data_dir().join("avatars"), // bridge.rs resolve_avatar's
        crate::memories_ctl::thumbs_dir(),
    ] {
        gone(std::fs::remove_dir_all(&dir), &dir);
    }
    for file in [
        crate::mentions_ctl::file(),
        crate::dice_ctl::file(),
        crate::widgets_ctl::cache_path(),
        crate::presence::tz_auto_file(),
    ] {
        gone(std::fs::remove_file(&file), &file);
    }
    let prefs = crate::settings::prefs_dir();
    if let Ok(entries) = std::fs::read_dir(&prefs) {
        let names: Vec<String> = entries.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect();
        for name in account_pref_files(names.iter().map(String::as_str)) {
            let f = prefs.join(name);
            gone(std::fs::remove_file(&f), &f);
        }
    }
    crate::room::forget_session();
    crate::mentions_ctl::forget_session();
    crate::dice_ctl::forget_session();
    crate::widgets_ctl::forget_session();
    crate::linkpreview::forget_session();
    eprintln!("[signout] the account's caches, lists and drafts are gone from this device");
}

fn gone(r: std::io::Result<()>, what: &std::path::Path) {
    if let Err(e) = r {
        if e.kind() != std::io::ErrorKind::NotFound {
            eprintln!("[signout] couldn't remove {}: {e}", what.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The web's account keys, by the native's names; every device setting stays.
    #[test]
    fn account_prefs_are_the_webs_account_keys() {
        let names = [
            "blob-chat-drafts",
            "blob-memories-resume",
            "ui-last-room",
            "ui-soundboard-recent",
            "ui-theme",
            "ui-main-w",
            "ui-reduce-motion",
            "ui-last-roomy",
            "call-input",
            "call-output",
            "push-to-talk",
            "logship",
            "boot-state",
            "media-key-index",
            "blobby",
        ];
        assert_eq!(
            account_pref_files(names),
            vec!["blob-chat-drafts", "blob-memories-resume", "ui-last-room", "ui-soundboard-recent"]
        );
    }
}
