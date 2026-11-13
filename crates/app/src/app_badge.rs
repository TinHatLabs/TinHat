//! The taskbar / dock unread badge (REACH A5.10, the desktop half; Tin Hat Web sets the same number with
//! `navigator.setAppBadge`). The number is how many rooms are waiting on you: rooms that are not spaces and are
//! either joined with a settings-adjusted unread (A5.3, `nav::adjusted_unread`) above 0, or a pending DM invite. One
//! room counts one whatever its number; no cap (the shell draws its own "99+" if it wants one). The rule is pure
//! ([`app_badge_rooms`]) and replays the shared vectors `testvectors/app-badge.json`, owned here; the web's
//! `src/features/nav/badge.ts` replays a byte copy.
//!
//! Where it shows:
//! * Linux: the `com.canonical.Unity.LauncherEntry` `Update` signal on the session bus (`count` + `count-visible`)
//!   for `application://dev.tinhat.TinHat.desktop`, the id of the Flatpak and of the packaged `.desktop` file (and the
//!   Wayland app_id, main.rs `set_desktop_file_name`). KDE Plasma's task manager and Ubuntu Dock / Dash to Dock read
//!   it. A broadcast signal needs no extra Flatpak permission (the D-Bus proxy lets signals without a destination
//!   out). Docks forget an entry when our bus connection goes, so quitting clears it.
//! * Windows: `ITaskbarList3::SetOverlayIcon` on each of our taskbar buttons, a red disc with the count drawn by
//!   [`render_overlay`] (cleared at 0). See `taskbar_win`.
//!
//! Settings: none, as on the web (its only gate is the browser's notification permission). The shell's own setting
//! turns it off (Plasma: the task manager's "Show badges"; Windows: Taskbar settings → "Show badges on taskbar apps").

/// A room row as the badge sees it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BadgeRow<'a> {
    pub id: &'a str,
    pub membership: Membership,
    pub is_space: bool,
    pub dm: bool,
    /// The row's settings-adjusted unread (A5.3); an invite's row shows 0.
    pub unread: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Membership {
    Join,
    Invite,
    /// Anything else (left, banned, knocked): never counts. The app only builds rows for joined rooms and invites;
    /// the shared vectors use it.
    #[cfg_attr(not(test), allow(dead_code))]
    Other,
}

/// The ids the badge counts, in input order (the web's `appBadgeRooms`).
pub(crate) fn app_badge_rooms<'a>(rows: &[BadgeRow<'a>]) -> Vec<&'a str> {
    rows.iter()
        .filter(|r| {
            !r.is_space
                && ((r.membership == Membership::Join && r.unread > 0) || (r.membership == Membership::Invite && r.dm))
        })
        .map(|r| r.id)
        .collect()
}

/// The badge number (the web's `appBadgeCount`).
pub(crate) fn app_badge_count(rows: &[BadgeRow<'_>]) -> u32 {
    u32::try_from(app_badge_rooms(rows).len()).unwrap_or(u32::MAX)
}

/// The corner / global badge text (A5.3 + A5.1; the web's `globalBadge`, nav/badge.ts): the sum of the joined
/// non-space rows' adjusted unread plus 1 per pending DM invite (like the People bubble), as badge text ("" at 0,
/// "99+" above 99). Shared vectors: each case's `global`.
pub(crate) fn global_badge(rows: &[BadgeRow<'_>]) -> String {
    let total: u64 = rows
        .iter()
        .filter(|r| !r.is_space)
        .map(|r| match r.membership {
            Membership::Join => r.unread,
            Membership::Invite if r.dm => 1,
            _ => 0,
        })
        .fold(0u64, u64::saturating_add);
    crate::nav::badge_text(total)
}

/// [`publish`] for a number computed off-thread for session `generation` (`session::generation()` read before the
/// computation started): dropped when that session has since ended, so a refresh still in flight at sign-out can't
/// put the old account's count back after sign-out cleared it.
pub(crate) fn publish_for(generation: u64, n: u32) {
    if crate::session::generation() == generation && crate::session::client().is_some() {
        publish(n);
    }
}

/// Show `n` on the taskbar / dock (0 clears it). Call it on the GUI thread (Windows sets the overlay there); cheap
/// to call on every nav refresh: an unchanged number is not re-sent more than once a minute (re-announced so a dock
/// started after us, or a restarted Windows Explorer, picks it up).
pub(crate) fn publish(n: u32) {
    #[cfg(target_os = "linux")]
    linux::publish(n);
    #[cfg(windows)]
    taskbar_win::publish(n);
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = n;
}

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};

    pub(super) const APP_URI: &str = "application://dev.tinhat.TinHat.desktop";
    pub(super) const PATH: &str = "/com/canonical/unity/launcherentry/tinhat";
    pub(super) const IFACE: &str = "com.canonical.Unity.LauncherEntry";
    const REANNOUNCE: Duration = Duration::from_secs(60);

    /// The number the app wants shown (the newest publish wins, whatever order the sends run in).
    static WANTED: AtomicU32 = AtomicU32::new(0);

    /// The last number sent and when; one send at a time.
    fn sent() -> &'static tokio::sync::Mutex<Option<(u32, Instant)>> {
        static S: OnceLock<tokio::sync::Mutex<Option<(u32, Instant)>>> = OnceLock::new();
        S.get_or_init(|| tokio::sync::Mutex::new(None))
    }

    pub(super) fn publish(n: u32) {
        WANTED.store(n, Ordering::SeqCst);
        crate::bridge::runtime().spawn(async {
            let mut last = sent().lock().await;
            let n = WANTED.load(Ordering::SeqCst);
            if let Some((m, at)) = *last {
                if m == n && at.elapsed() < REANNOUNCE {
                    return;
                }
            }
            if last.is_none() && n == 0 {
                return; // nothing shown yet: nothing to clear
            }
            let Some(conn) = crate::notify::connection().await else { return };
            match send(&conn, n).await {
                Ok(()) => {
                    if last.map(|(m, _)| m) != Some(n) {
                        eprintln!("[badge] dock count {n}");
                    }
                    *last = Some((n, Instant::now()));
                }
                Err(e) => eprintln!("[badge] LauncherEntry.Update failed: {e}"),
            }
        });
    }

    /// The `Update(s app_uri, a{sv} properties)` broadcast.
    pub(super) async fn send(conn: &zbus::Connection, n: u32) -> zbus::Result<()> {
        let mut props: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();
        props.insert("count", zbus::zvariant::Value::from(i64::from(n)));
        props.insert("count-visible", zbus::zvariant::Value::from(n > 0));
        conn.emit_signal(None::<&str>, PATH, IFACE, "Update", &(APP_URI, props)).await
    }
}

/// The Windows overlay's pixels: a `size`×`size` BGRA image with straight (not premultiplied) alpha, row 0 = top — what
/// `CreateIconIndirect` takes for a 32-bit icon — a red disc with the count in white ("99+" above 99). Pure, so it is
/// tested on every platform.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn render_overlay(n: u32, size: usize) -> Vec<[u8; 4]> {
    // 3×5 digits (and '+'), one bit per pixel, top row first, high bit = left
    const GLYPHS: [[u8; 5]; 11] = [
        [0b111, 0b101, 0b101, 0b101, 0b111], // 0
        [0b010, 0b110, 0b010, 0b010, 0b111], // 1
        [0b111, 0b001, 0b111, 0b100, 0b111], // 2
        [0b111, 0b001, 0b111, 0b001, 0b111], // 3
        [0b101, 0b101, 0b111, 0b001, 0b001], // 4
        [0b111, 0b100, 0b111, 0b001, 0b111], // 5
        [0b111, 0b100, 0b111, 0b101, 0b111], // 6
        [0b111, 0b001, 0b010, 0b010, 0b010], // 7
        [0b111, 0b101, 0b111, 0b101, 0b111], // 8
        [0b111, 0b101, 0b111, 0b001, 0b111], // 9
        [0b000, 0b010, 0b111, 0b010, 0b000], // +
    ];
    let mut px = vec![[0u8; 4]; size * size];
    if n == 0 || size < 8 {
        return px;
    }
    let text: Vec<usize> = if n > 99 {
        vec![9, 9, 10]
    } else {
        n.to_string().bytes().map(|b| usize::from(b - b'0')).collect()
    };
    // the disc, anti-aliased by 4×4 supersampling; Windows' badge red (#C42B1C)
    let (r0, g0, b0) = (0xC4u32, 0x2Bu32, 0x1Cu32);
    let c = size as f32 / 2.0;
    let rad = c;
    for y in 0..size {
        for x in 0..size {
            let mut cover = 0u32;
            for sy in 0..4 {
                for sx in 0..4 {
                    let fx = x as f32 + (sx as f32 + 0.5) / 4.0 - c;
                    let fy = y as f32 + (sy as f32 + 0.5) / 4.0 - c;
                    if fx * fx + fy * fy <= rad * rad {
                        cover += 1;
                    }
                }
            }
            let a = cover * 255 / 16;
            px[y * size + x] = if a == 0 { [0, 0, 0, 0] } else { [b0 as u8, g0 as u8, r0 as u8, a as u8] };
        }
    }
    // the text: the biggest whole-pixel scale whose box fits ~70 % of the disc's width and ~60 % of its height
    let cols = text.len() * 3 + (text.len() - 1); // 1 px gap between glyphs
    let scale = ((size * 7 / 10) / cols).min((size * 6 / 10) / 5).max(1);
    let (w, h) = (cols * scale, 5 * scale);
    let (ox, oy) = ((size.saturating_sub(w)) / 2, (size.saturating_sub(h)) / 2);
    for (i, g) in text.iter().enumerate() {
        let gx = ox + i * 4 * scale;
        for (row, bits) in GLYPHS[*g].iter().enumerate() {
            for col in 0..3 {
                if bits & (0b100 >> col) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let (x, y) = (gx + col * scale + dx, oy + row * scale + dy);
                        if x < size && y < size {
                            px[y * size + x] = [255, 255, 255, 255];
                        }
                    }
                }
            }
        }
    }
    px
}

#[cfg(windows)]
#[path = "app_badge_win.rs"]
mod taskbar_win;

#[cfg(test)]
mod tests {
    use super::*;
    use matrix_sdk_base::notification_settings::RoomNotificationMode as M;

    const VECTORS: &str = include_str!("testvectors/app-badge.json");

    fn mode(s: &str) -> M {
        match s {
            "all" => M::AllMessages,
            "mentions" => M::MentionsAndKeywordsOnly,
            "mute" => M::Mute,
            other => panic!("unknown mode {other}"),
        }
    }

    /// Replays every shared case: each row's unread is the A5.3 adjusted number (0 for an invite), as build_nav
    /// builds it. The web's badge.test.ts runs the same file.
    fn replay(rule: fn(&[BadgeRow<'_>]) -> Vec<String>) -> Vec<String> {
        replay_with(rule, global_badge)
    }

    /// The same replay, with the global badge rule given too (every case has a `global`).
    fn replay_with(rule: fn(&[BadgeRow<'_>]) -> Vec<String>, global_rule: fn(&[BadgeRow<'_>]) -> String) -> Vec<String> {
        let v: serde_json::Value = serde_json::from_str(VECTORS).unwrap();
        assert_eq!(v["v"], 1);
        let cases = v["cases"].as_array().unwrap();
        assert!(cases.len() >= 10);
        let mut failed = Vec::new();
        for c in cases {
            let rows_v = c["rows"].as_array().unwrap();
            let rows: Vec<BadgeRow<'_>> = rows_v
                .iter()
                .map(|r| {
                    let membership = match r["membership"].as_str().unwrap() {
                        "join" => Membership::Join,
                        "invite" => Membership::Invite,
                        _ => Membership::Other,
                    };
                    let is_space = r["space"].as_bool().unwrap();
                    let unread = if membership == Membership::Join {
                        crate::nav::adjusted_unread(
                            mode(r["mode"].as_str().unwrap()),
                            r["notif"].as_u64().unwrap(),
                            r["highlight"].as_u64().unwrap(),
                            is_space,
                        )
                    } else {
                        0
                    };
                    BadgeRow { id: r["id"].as_str().unwrap(), membership, is_space, dm: r["dm"].as_bool().unwrap(), unread }
                })
                .collect();
            let want: Vec<String> =
                c["want"].as_array().unwrap().iter().map(|w| w.as_str().unwrap().to_string()).collect();
            let got = rule(&rows);
            if got != want {
                failed.push(format!("{}: got {got:?}, want {want:?}", c["name"].as_str().unwrap()));
            }
            if let Some(g) = c["global"].as_str() {
                let got_g = global_rule(&rows);
                if got_g != g {
                    failed.push(format!("{} (global): got {got_g:?}, want {g:?}", c["name"].as_str().unwrap()));
                }
            }
        }
        failed
    }

    fn real(rows: &[BadgeRow<'_>]) -> Vec<String> {
        assert_eq!(app_badge_count(rows) as usize, app_badge_rooms(rows).len());
        app_badge_rooms(rows).into_iter().map(String::from).collect()
    }

    #[test]
    fn app_badge_shared_vectors() {
        let failed = replay(real);
        assert!(failed.is_empty(), "app-badge vectors failed:\n{}", failed.join("\n"));
    }

    /// Negative controls: the replay must catch a rule that forgets DM invites, one that counts room invites, and
    /// one that sums numbers instead of counting rooms (the dock/corner badge's rule, not the app badge's).
    #[test]
    fn app_badge_vectors_catch_wrong_rules() {
        fn no_dm_invites(rows: &[BadgeRow<'_>]) -> Vec<String> {
            rows.iter()
                .filter(|r| !r.is_space && r.membership == Membership::Join && r.unread > 0)
                .map(|r| r.id.to_string())
                .collect()
        }
        fn all_invites(rows: &[BadgeRow<'_>]) -> Vec<String> {
            rows.iter()
                .filter(|r| !r.is_space && ((r.membership == Membership::Join && r.unread > 0) || r.membership == Membership::Invite))
                .map(|r| r.id.to_string())
                .collect()
        }
        fn summed(rows: &[BadgeRow<'_>]) -> Vec<String> {
            let mut out = Vec::new();
            for r in rows.iter().filter(|r| !r.is_space) {
                let k = if r.membership == Membership::Invite && r.dm { 1 } else if r.membership == Membership::Join { r.unread } else { 0 };
                for _ in 0..k {
                    out.push(r.id.to_string());
                }
            }
            out
        }
        assert!(!replay(no_dm_invites).is_empty());
        assert!(!replay(all_invites).is_empty());
        assert!(!replay(summed).is_empty());
        // the global badge: a rule that leaves DM invites out (the desktop's old build_nav / global_badges) or that
        // also counts room invites must fail
        fn global_no_dm_invites(rows: &[BadgeRow<'_>]) -> String {
            crate::nav::badge_text(rows.iter().filter(|r| !r.is_space && r.membership == Membership::Join).map(|r| r.unread).sum())
        }
        fn global_all_invites(rows: &[BadgeRow<'_>]) -> String {
            crate::nav::badge_text(
                rows.iter()
                    .filter(|r| !r.is_space)
                    .map(|r| match r.membership { Membership::Join => r.unread, Membership::Invite => 1, Membership::Other => 0 })
                    .sum(),
            )
        }
        assert!(!replay_with(real, global_no_dm_invites).is_empty());
        assert!(!replay_with(real, global_all_invites).is_empty());
    }

    #[test]
    fn every_case_has_a_global_badge() {
        let v: serde_json::Value = serde_json::from_str(VECTORS).unwrap();
        for c in v["cases"].as_array().unwrap() {
            assert!(c["global"].is_string(), "{} has no global", c["name"]);
        }
    }

    /// The web keeps a byte copy; when its checkout is known (TINHAT_WEB_DIR), the bytes must match.
    #[test]
    fn app_badge_vectors_match_the_web_copy() {
        let Ok(dir) = std::env::var("TINHAT_WEB_DIR") else { return };
        let path = std::path::Path::new(&dir).join("src/features/nav/testvectors/app-badge.json");
        let theirs = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(VECTORS == theirs, "testvectors/app-badge.json differs from {}", path.display());
    }

    #[test]
    fn overlay_render() {
        // 0 = fully transparent (the overlay is cleared, never drawn)
        assert!(render_overlay(0, 16).iter().all(|p| p[3] == 0));
        for (n, size) in [(1u32, 16usize), (7, 16), (42, 16), (99, 20), (100, 16), (12345, 32), (5, 32)] {
            let px = render_overlay(n, size);
            assert_eq!(px.len(), size * size);
            let white = px.iter().filter(|p| **p == [255, 255, 255, 255]).count();
            assert!(white > 0, "{n}@{size}: no text");
            // corners are outside the disc; the centre row has the disc or text
            assert_eq!(px[0][3], 0, "{n}@{size}: corner drawn");
            assert!(px[(size / 2) * size + 1][3] > 0, "{n}@{size}: no disc");
            // straight alpha: every visible pixel is the badge red or white, transparent ones are all zero
            assert!(px.iter().all(|p| p[3] == 0 && *p == [0, 0, 0, 0] || (p[0], p[1], p[2]) == (0x1C, 0x2B, 0xC4) || *p == [255, 255, 255, 255]));
        }
        // different numbers look different; everything above 99 is the same "99+"
        assert_ne!(render_overlay(1, 16), render_overlay(7, 16));
        assert_ne!(render_overlay(42, 16), render_overlay(24, 16));
        assert_eq!(render_overlay(100, 16), render_overlay(5000, 16));
        assert_ne!(render_overlay(99, 16), render_overlay(100, 16));
    }

    /// The Linux round on the session bus through the app's own path (`publish` → the shared notification
    /// connection → the signal), a stand-in dock listening: run with `TINHAT_BADGE_BUS_TEST=1` under
    /// `dbus-run-session` (scripts/app-badge-dbus-round.sh). `TINHAT_BADGE_BUS_COUNTS` (default `3,3,0`) is published
    /// in order, 300 ms apart; an unchanged number is not re-sent, so the dock sees 3 then 0.
    #[cfg(target_os = "linux")]
    #[test]
    fn launcher_entry_signal_on_the_bus() {
        if std::env::var("TINHAT_BADGE_BUS_TEST").is_err() {
            return;
        }
        let counts: Vec<u32> = std::env::var("TINHAT_BADGE_BUS_COUNTS")
            .ok()
            .map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect())
            .unwrap_or_else(|| vec![3, 3, 0]);
        for n in counts {
            publish(n);
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    }
}
