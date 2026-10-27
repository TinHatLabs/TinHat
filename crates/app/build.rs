// CXX-Qt 0.8 build: register the QML module + every Rust bridge file.
// Each #[qml_element] QObject auto-inherits this module's URI; all bridge files
// must be registered on the SAME new_qml_module builder so they share the URI.
use cxx_qt_build::{CxxQtBuilder, QmlFile, QmlModule};

// themes (desktop C1): the Th singleton's shape — shared with the app, which fills it at run time.
#[allow(dead_code)]
#[path = "src/theme_gen.rs"]
mod theme_gen;

/// themes (desktop C1): write qml/Th.qml from crates/theme/tokens.json + the built-in OLED when it is stale, so no
/// binary can ever carry a singleton that disagrees with the engine's token list (docs/MIGRATING-THEME.md). The
/// tinhat-theme build-dependency embeds tokens.json and the built-ins, so editing them re-runs this.
fn generate_th_qml() {
    let tokens = tinhat_theme::parse_json(tinhat_theme::engine::TOKENS_JSON).expect("crates/theme/tokens.json is JSON");
    let entries = theme_gen::entries(&tokens).unwrap_or_else(|e| panic!("tokens.json cannot become qml/Th.qml: {e}"));
    let doc = tinhat_theme::builtin_theme("oled").expect("the built-in OLED theme");
    let oled = tinhat_theme::resolve_for_app(doc, tinhat_theme::app_tokens(), tinhat_theme::App::Desktop)
        .unwrap_or_else(|e| panic!("the built-in OLED theme does not resolve: {e}"));
    let name = doc.get("name").and_then(|v| v.as_str()).unwrap_or("OLED");
    let text = theme_gen::th_qml(&entries, &oled, ("oled", name, oled.variant.as_str(), oled.names.as_str()));
    let path = std::path::Path::new(theme_gen::TH_FILE);
    if std::fs::read_to_string(path).ok().as_deref() != Some(text.as_str()) {
        std::fs::write(path, &text).unwrap_or_else(|e| panic!("{} is stale and cannot be rewritten: {e}", path.display()));
        println!("cargo:warning={} regenerated from crates/theme/tokens.json — commit it", path.display());
    }
}

/// H3 dev-key segregation (security review follow-up, 2026-10-26): the updater trusts a development key only in a LAB
/// build — the `lab-update-key` feature AND `TINHAT_UPDATE_DEV_KEY` (a minisign public key) together; either one alone
/// stops the build (a stray variable in a shell can't slip a key into an ordinary build), and so does the feature when
/// the build says it is a release (`TINHAT_RELEASE_BUILD=1`, set by the release trains). A lab binary carries a fixed
/// marker the publish step refuses (src/update_trust.rs LAB_TRUST_MARKER).
fn check_update_trust_config() {
    println!("cargo:rerun-if-env-changed=TINHAT_UPDATE_DEV_KEY");
    println!("cargo:rerun-if-env-changed=TINHAT_RELEASE_BUILD");
    let lab = std::env::var_os("CARGO_FEATURE_LAB_UPDATE_KEY").is_some();
    let key = std::env::var("TINHAT_UPDATE_DEV_KEY").ok().filter(|k| !k.trim().is_empty());
    let release = std::env::var("TINHAT_RELEASE_BUILD").map(|v| v.trim() == "1").unwrap_or(false);
    match (lab, &key) {
        (false, Some(_)) => panic!(
            "TINHAT_UPDATE_DEV_KEY is set, but this is not a lab build: unset it (a lab build adds --features lab-update-key)"
        ),
        (true, None) => panic!("--features lab-update-key needs TINHAT_UPDATE_DEV_KEY (the development minisign public key)"),
        _ => {}
    }
    if let Some(k) = key {
        let k = k.trim();
        if k.len() != 56 || !k.starts_with("RW") || !k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/') {
            panic!("TINHAT_UPDATE_DEV_KEY is not a minisign public key (the base64 line of a .pub file)");
        }
        if release {
            panic!("TINHAT_RELEASE_BUILD=1 with --features lab-update-key: a release never trusts a development update key");
        }
    }
}

fn main() {
    check_update_trust_config();
    generate_th_qml();

    // Bake a short git hash so dev vs released builds are distinguishable (About sheet +
    // the auto-updater). Best-effort: falls back to "dev" if git/.git is unavailable.
    let hash = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(h) = hash {
        println!("cargo:rustc-env=TINHAT_BUILD_HASH={h}");
    }
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    // 0.8.35: every QML edit must reach the binary — without this the script only re-ran on a
    // version bump or a C++ bridge change, and QML edited after the bump was silently stale.
    println!("cargo:rerun-if-changed=qml");

    // MSVC assumes the *system ANSI codepage* for narrow string literals, but
    // qmlcachegen emits our QML strings as narrow UTF-8 inside QStringLiteral (which
    // expands to `u"" "..."`). Without /utf-8 every non-ASCII character in the UI is
    // transcoded from CP1252 and reaches users as mojibake — "Settings → Privacy"
    // rendered as "Settings â†' Privacy". GCC/Clang default to UTF-8, which is
    // exactly why this was Windows-only and invisible on the Linux build.
    //
    // SAFETY: the closure only appends a compiler flag; no cc::Build invariant is touched.
    unsafe {
        CxxQtBuilder::new_qml_module(
            QmlModule::new("dev.tinhat.app").qml_files([
                "qml/Main.qml",
                "qml/AppCore.qml",
                "qml/GameReelPanel.qml",
                "qml/GameReelWindow.qml",
                "qml/ReelCover.qml",
                "qml/ReelButton.qml",
                "qml/TierList.qml",
                "qml/MyGamesEditor.qml",
                "qml/GameNightAdmin.qml",
                "qml/StartupWindow.qml",
                "qml/FallbackTheme.qml",
                // themes step 1b (contract THEMES-UI.md): Settings → Appearance, the gallery, cards, import
                "qml/AppearancePane.qml",
                "qml/EditMode.qml",   // 11.0 edit mode (EDITMODE-UI.md): the state + actions
                "qml/EditLayer.qml",  //   the chrome over the dock host
                "qml/EditToolbar.qml", //  the toolbar strip + its panels
                "qml/LayoutSection.qml", // Settings → Appearance → Layout (named layouts, reset, previous)
                "qml/ThemeSection.qml",
                "qml/ThemeNotices.qml",
                "qml/ThemeSummary.qml",
                "qml/ThemeGrid.qml",
                "qml/ThemeCard.qml",
                "qml/ThemeTile.qml",
                "qml/ThemeGalleryPage.qml",
                // 11.0 step 5 (THEMES-UI.md §13–§15): the theme editor, the share cards, the Pick tool, the review harness
                "qml/ThemeEditorPanel.qml",
                "qml/ThemeEditorSimple.qml",
                "qml/ThemeEditorAdvanced.qml",
                "qml/ThemeEditorIconButton.qml",
                "qml/ThemeEditorSwatch.qml",
                "qml/ThemeEditorWarnings.qml",
                "qml/ThemeColourPopover.qml",
                "qml/ThemePickerHandle.qml",
                "qml/ThemeChecker.qml",
                "qml/ThemePickOverlay.qml",
                "qml/ThemeShareCard.qml",
                "qml/ThemeShareButton.qml",
                "qml/ThemeTryBar.qml",
                "qml/ThemeEditorGallery.qml",
                "qml/ThemeImportDialog.qml",
                "qml/ThemeDropOverlay.qml",
                "qml/ThemeSwatch.qml", // THEMES-UI §12: the ☰ Theme menu's swatch
                "qml/UnreadBadge.qml",
                "qml/SectionLabel.qml",
                "qml/HintLabel.qml",
                "qml/CheckMenuItem.qml",
                "qml/MenuSectionLabel.qml",
                "qml/Avatar.qml",
                "qml/ChatBubble.qml",
                "qml/FoxScrollBar.qml",
                "qml/GlowButton.qml",
                "qml/RowLayoutShim.qml",
                "qml/LoginField.qml",
                "qml/LoginView.qml",
                "qml/HeaderIconButton.qml",
                "qml/RoomHeaderBar.qml",
                "qml/EmptyState.qml",
                "qml/ConfirmDialog.qml",
                "qml/TextViewerDialog.qml",   // launch-gate's web review #8: text-like attachments shown in the app
                "qml/ChangePasswordDialog.qml",   // selfhost: Settings → Devices & security → Password (password.rs)
                "qml/BugReportDialog.qml",
                "qml/VerifyDeviceDialog.qml",
                "qml/EmojiPicker.qml",
                "qml/QuickSwitcher.qml",
                // 11.0 step 3 (buttons): action buttons, button rows, the id → command map, the review captures
                "qml/ActionButton.qml",
                "qml/ButtonRow.qml",
                "qml/ActionsHost.qml",
                "qml/ButtonsTest.qml",
                // 11.0 buttons: rows from the table (RowModel), this device's layout form (LayoutStore), the row editor,
                // your own button islands, Settings → Appearance → Buttons
                "qml/RowModel.qml",
                "qml/LayoutStore.qml",
                "qml/RowEditor.qml",
                "qml/ButtonsIsland.qml",
                "qml/ButtonsSettings.qml",
                "qml/ShortcutsPane.qml",
                "qml/RightPanel.qml",
                "qml/ThEmbedCard.qml",
                "qml/LinkPreviewCard.qml",
                "qml/ThDock.qml",
                "qml/ThSpaceRail.qml",
                "qml/ThRoomList.qml",
                "qml/ThNav.qml",
                "qml/Tour.qml",
                "qml/SoundboardPopover.qml",
                "qml/WatchPartyWindow.qml",
                "qml/MemoriesWindow.qml",
                "qml/ConversationView.qml",
                "qml/FloatingChats.qml",
                "qml/PipWindow.qml",
                "qml/StreamPopover.qml",
                "qml/Lightbox.qml",
                "qml/OverlayWindow.qml",
                "qml/CamGlTest.qml",
                "qml/ThIcon.qml", // 0.8.26: icon-pack aware icon (static theme or animated webp)
                "qml/CalendarWindow.qml", // 0.8.26: squad calendar
                "qml/IconGalleryWindow.qml", // 0.8.26: icon-pack review gallery
                "qml/TinHatLoader.qml", // 10.0: the Tin Hat mark as a busy indicator (vector shimmer)
                "qml/IncomingCallToast.qml", // 10.0: incoming-call card (join / switch / dismiss)
                "qml/CornerMask.qml", // 0.8.30: rounded-corner bites for surfaces a clip cannot round (video)
                "qml/ThToolButton.qml", // 0.8.30: ToolButton drawn with the icon pack (hover stories)
                "qml/ParticipantVolumeMenu.qml", // 0.8.31: per-person volume menu shared by the left bar + call tiles
                "qml/CalendarCard.qml", // 0.8.33: live calendar invite card in chat bubbles
                "qml/EcOutputsPopup.qml", // 0.8.33: which outputs the echo canceller listens to
                "qml/CallOptionsPopover.qml", // 0.8.33: the call bar chevrons' slide-out settings
                "qml/ThComboBox.qml", // 0.8.33: house controls — dark, identical on both platforms
                "qml/ThTextField.qml",
                "qml/ThTextArea.qml",
                "qml/ThCheckBox.qml",
                "qml/ThSwitch.qml", // fix/basic-switch-on: every switch — the checked track reads toggle.track.checked (Basic drew palette.dark)
                "qml/ThSlider.qml", // fix/sliders-volume: every slider — fill / groove / handle read slider.* (Basic drew the fill in palette.dark)
                "qml/ThMenu.qml", // fix/sliders-volume: a Menu as wide as its widest row (Basic's is a fixed 200 px) — the menus with slider rows
                "qml/ThRadioButton.qml",
                "qml/ThButton.qml",
                "qml/ThBusyIndicator.qml", // 11.0 (GPU.md §2.5): the house busy mark — still, one step per progress event
                "qml/ThGif.qml", // 11.0 (GPU.md §2.6): GIFs / animated WebP — ≤ 3 loops or 15 s while focused + in view
                "qml/WindowMemory.qml", // 10.1: remembered window size/position (TH-SHELL-05 & co.)
                "qml/SpaceSettingsWindow.qml", // RBAC: a space's Overview / Members / Roles / Rooms (docs/RBAC.md §7.2)
                "qml/SettingsWindow.qml", // the Settings window (contract SETTINGS-WINDOW.md, docs/SETTINGS-WINDOW.md)
                "qml/SettingsPage.qml",   //   a pane: the scroll area, the header, the 720 px column
                "qml/SettingsSection.qml", //  a section: heading, hint, card
                "qml/SettingsCard.qml",   //   a card of rows
                "qml/SettingsRow.qml",    //   a row: label, description, control; the search anchor + flash
                "qml/SettingsRowGroup.qml", // rows from another component in the same card
                "qml/LevelMeter.qml",     //   the house level meter (was Main.qml's inline component)
                "qml/RoleBadge.qml", // RBAC: the one role chip (members list, RightPanel)
                "qml/SpaceInviteCard.qml", // RBAC: invited-space card in the room panel (§7.3)
                "qml/StatusChip.qml", // 10.6: a custom status pill (docs/REACH.md A4.5)
                "qml/PresenceLine.qml", // 10.6: the profile card's presence line (REACH §2.6 + A4.5)
                // widgets (modular UI step 2, contract WIDGETS.md + WIDGETS-UI.md): the stack under the room list
                "qml/WidgetHost.qml",
                "qml/WidgetFrame.qml",
                "qml/WidgetGallery.qml",
                "qml/WidgetSettingsDialog.qml",
                "qml/WidgetText.qml",
                "qml/WidgetBusyMark.qml",
                "qml/WidgetRing.qml",
                "qml/WidgetSparkline.qml",
                "qml/WidgetCover.qml",
                "qml/UpNextWidget.qml",
                "qml/CallStatsWidget.qml",
                "qml/WhosAroundWidget.qml",
                // 11.1: the Game servers widget + its pieces Up next and the calendar card reuse (docs/GAMESERVERS.md)
                "qml/GameServersWidget.qml",
                "qml/GameServerIconButton.qml",
                "qml/GameServerLine.qml",
                "qml/GameServerPower.qml",
                "qml/PinnedWidget.qml", // widgets W1 (2026-09-28): Pinned
                "qml/WidgetHeaderText.qml", // widgets W1: an elided header line whose implicitWidth is the text's own
                "qml/MentionsWidget.qml", // widgets W1: Mentions
                "qml/NoteWidget.qml", // widgets W1: Sticky note
                "qml/ClocksWidget.qml", // widgets W1: Friends' clocks
                "qml/DiceWidget.qml", // widgets W3: Dice
                "qml/DiceCard.qml", // widgets W3: a roll as a card (the widget's last roll, the chat card)
                "qml/PollsWidget.qml", // widgets W3: Polls
                "qml/PollCard.qml", // widgets W3: a poll as a card (the chat)
                "qml/NewPollDialog.qml", // widgets W3: the New poll dialog
                "qml/JustMeWidget.qml", // widgets W2: "Is it just me?" (WIDGETS.md §20)
                "qml/MemoriesWidget.qml", // widgets W2: Memories (WIDGETS.md §21)
                "qml/SoundPadWidget.qml", // widgets W2: the Soundboard pad (WIDGETS.md §22)
                "qml/GameNightWidget.qml", // widgets W2: Game night (WIDGETS.md §23)
                "qml/WidgetPeople.qml", // widgets W2: who's going (moved out of UpNextWidget, shared with Game night)
                "qml/WidgetRsvp.qml", // widgets W2: the RSVP buttons (moved out of UpNextWidget, shared with Game night)
                "qml/WidgetAnswerMark.qml", // widgets W2: ✓ ? ✕ (moved out of UpNextWidget)
                // islands (modular UI, ISLANDS-ARCH): the islands, extracted from Main.qml (C3), then placed by the dock host
                "qml/HeaderIsland.qml",
                "qml/SpaceBubble.qml", // islands C3: the rail's bubble (was ThSpaceRail's inline component)
                "qml/NavIsland.qml",
                "qml/DmsIsland.qml",
                "qml/SpacesIsland.qml",
                "qml/RoomsIsland.qml",
                "qml/AccountIsland.qml",
                "qml/CallButton.qml", // islands C3: the call bar's round button (was Main.qml's inline component)
                "qml/HearPill.qml", // islands C3: the "can they hear me?" pill (was Main.qml's inline component)
                "qml/CallIsland.qml", // islands C3: the in-call panel + the call/chat seam
                "qml/Island.qml", // islands C4: an island's frame in the dock host
                "qml/DockHost.qml", // islands C4: the flat dock host (solve + one imperative apply; episodes; focus recovery)
                "qml/HostTest.qml", // islands C4: the host's offscreen test (TINHAT_HOST_TEST)
                // Pad (docs/PAD.md): the receiver's executor + snapshot, the Pad window and its pieces, the room-list bar
                "qml/PadHost.qml",
                "qml/PadWindow.qml",
                "qml/PadSoundButton.qml",
                "qml/PadVolumeMenu.qml",
                "qml/PadBar.qml",
                "qml/PadChip.qml",
            ])
            // themes (desktop C1): the active theme's colours, generated above (docs/MIGRATING-THEME.md).
            .qml_file(QmlFile::from(theme_gen::TH_FILE).singleton(true))
            // 11.0 (GPU.md §1): the app-wide window state + motion switch + the duration token (AppCore binds it).
            .qml_file(QmlFile::from("qml/Motion.qml").singleton(true))
            // selfhost/firstrun §6: one colour per person (the web's avatarSeed.ts)
            .qml_file(QmlFile::from("qml/Seeds.qml").singleton(true))
            // selfhost/firstrun §4: an invite link pasted into the sign-in screen (the web's invite.ts)
            .qml_file(QmlFile::from("qml/InviteLink.qml").singleton(true))
            // security review 2026-10-26: the one door every link / attachment the app hands to the OS goes through
            .qml_file(QmlFile::from("qml/LinkGate.qml").singleton(true))
            // 11.0 step 5: the theme editor's pure helpers (targets + colour maths; the web's targets.ts + colour.ts)
            .qml_file(QmlFile::from("qml/ThemeEditorLib.qml").singleton(true)),
        )
        .file("src/bridge.rs") // LoginController
        .file("src/boot.rs") // BootController (startup window stages, 0.8.24)
        .file("src/stream.rs") // StreamController
        .file("src/room.rs") // RoomController
        .file("src/call_ctl.rs") // CallController (full call: join/mic/screen)
        .file("src/settings.rs") // SettingsController (account profile / devices / security)
        .file("src/diagnostics.rs") // DiagnosticsController (opt-in redacted bug report)
        .file("src/update.rs") // UpdateController (in-app / startup update check + self-replace)
        .file("src/activity.rs") // GameActivityController ("what we're playing" per room)
        .file("src/audio_prefs.rs") // AudioPrefsController (per-(category,user) call volume/mute, #5)
        .file("src/status_ctl.rs") // UserStatusController (per-person "looking to play" status)
        .file("src/gif.rs") // GifController (in-chat Giphy picker via the server's Tin Hat API)
        .file("src/soundboard.rs") // SoundboardController (room + personal sounds, web search)
        .file("src/nav.rs") // NavController (corner-dock: spaces -> rooms drill-down)
        .file("src/watchparty.rs") // WatchPartyController (queue + sync-clock + viewer presence + search)
        .file("src/afk_ctl.rs") // AfkController (#7 BRB/AFK presence over Matrix reactions/state)
        .file("src/floating.rs") // FloatingController (#6 floating chat-bubble registry + metadata)
        .file("src/cam_gltest.rs") // CamGlTest (TINHAT_CAM_GLTEST camera GL-upload self-test harness)
        .file("src/pipvideo.rs") // PipVideoItem (PiP pop-out CPU video surface — no GstGL)
        .file("src/calendar.rs") // CalendarController (squad calendar in the space, 0.8.26)
        .file("src/games.rs") // GamesController (Game Reel: game-night picker in the space, 9.0)
        .file("src/iconpack.rs") // IconPackController (animated icon pack discovery, 0.8.26)
        .file("src/space.rs") // SpaceController (RBAC: space settings, members, roles, rooms)
        .file("src/invites.rs") // InviteController (10.4: registration invite codes for operators)
        .file("src/guests_ctl.rs") // GuestsController (selfhost: the guest room — Settings → Guests, the bar, the notices)
        .file("src/presence.rs") // PresenceController (10.6: opt-in presence + "at the computer" pings, docs/REACH.md)
        .file("src/theme_ctl.rs") // ThemeController (themes, desktop C1: the engine → the Th singleton)
        .file("src/theme_editor.rs") // ThemeEditorController (11.0 step 5: the theme editor, share cards, the Pick tool)
        .file("src/callstats_ctl.rs") // CallStatsController (widgets, step 2: the Call stats widget's live data)
        .file("src/widgets_ctl.rs") // WidgetsController (widgets, step 2: call history, Up next's events, the shared formatters)
        .file("src/mentions_ctl.rs") // MentionsController (widgets W1: the Mentions widget's per-device highlight list)
        .file("src/note_ctl.rs") // NoteController (widgets W1: the Sticky note — the space's dev.tinhat.note)
        .file("src/dice_ctl.rs") // DiceController (widgets W3: rolling with the OS CSPRNG, private rolls, the Dice history)
        .file("src/polls_ctl.rs") // PollsController (widgets W3: the Polls widget's Small line + samples; the poll card's JSON)
        .file("src/gameservers_ctl.rs") // GameServersController (11.1: the Game servers widget's data, docs/GAMESERVERS.md)
        .file("src/justme_ctl.rs") // JustMeController (widgets W2: "Is it just me?" — the checker, WIDGETS.md §20)
        .file("src/memories_ctl.rs") // MemoriesController (widgets W2: the Memories widget's data, WIDGETS.md §21)
        .file("src/gamenight_ctl.rs") // GameNightController (widgets W2: Game night's rules + my Reel credit, WIDGETS.md §23)
        .file("src/lowgpu_ctl.rs") // LowGpuController (step 3: the low-GPU contract's rules for QML, GPU.md)
        .file("src/layout_ctl.rs") // LayoutController (islands C1: the layout engine, JSON in / JSON out; no UI yet)
        .file("src/actions_ctl.rs") // ActionsController (11.0 step 3: the actions engine — rows, the palette, JSON in / out)
        .file("src/pad_ctl.rs") // PadController (Pad: the phone as a remote for my call — docs/PAD.md)
        .cc_builder(|cc| {
            if cc.get_compiler().is_like_msvc() {
                cc.flag("/utf-8");
            }
        })
        .build();
    }

    // Native remote-video tiles: compile the C++ qml6glsink bridge. cxx-qt links
    // Qt Core/Gui/Qml; we additionally pull Qt6Quick (QQuickWindow/QQuickItem) +
    // GStreamer (gst_*) here so the shim both compiles (include paths) and links
    // (pkg-config emits the cargo:rustc-link-lib directives).
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .file("src/video_bridge.cpp")
        .file("src/webengine_bridge.cpp") // QtWebEngineQuick::initialize() shim (inline embeds)
        .file("src/clipboard_bridge.cpp") // Ctrl+V attachments: clipboard image/files (9.3.2)
        .file("src/theme_bridge.cpp") // 11.0 step 5: the theme editor review harness's window grab
        .std("c++17")
        .flag_if_supported("-fPIC"); // no-op on MSVC
    // Same MSVC narrow-literal hazard as the QML units above: keep the shim's
    // own string literals on UTF-8 so nothing it surfaces to the UI can be mojibaked.
    if build.get_compiler().is_like_msvc() {
        build.flag("/utf-8");
    }

    #[cfg(target_os = "linux")]
    {
        // Qt6WebEngineQuick: include paths for the init shim + (via pkg-config metadata) the
        // link directives for libQt6WebEngineQuick / libQt6WebEngineCore. `mpv` (the watch-party
        // player surface's libmpv) emits its own -lmpv link directive here too.
        // Qt6Multimedia + gstreamer-video-1.0: the QVideoSink tile path (0.8.19, Memories
        // side tiles) — QVideoFrame/QVideoSink + gst_video_frame_map.
        for pkg in ["Qt6Quick", "Qt6Qml", "Qt6WebEngineQuick", "Qt6Multimedia", "gstreamer-1.0", "gstreamer-video-1.0", "mpv"] {
            let lib = pkg_config::Config::new()
                .probe(pkg)
                .unwrap_or_else(|e| panic!("pkg-config {pkg} failed: {e}"));
            for inc in &lib.include_paths {
                build.include(inc);
            }
        }
        // KF6 KColorScheme (for full-app theming via KColorSchemeManager) ships no
        // pkg-config file here, so add its include dirs + link libs manually.
        // (/usr/include/KF6/KConfig: kconfig_version.h, which KSharedConfig needs — themes 1b's scheme-file activation)
        for inc in ["/usr/include/KF6", "/usr/include/KF6/KColorScheme", "/usr/include/KF6/KConfigCore", "/usr/include/KF6/KConfigGui", "/usr/include/KF6/KConfig"] {
            build.include(inc);
        }
        // mpvqt (the watch-party player surface — MpvAbstractItem) ships a cmake target but NO
        // pkg-config, so add its include dirs + link lib manually. `<MpvQt/MpvAbstractItem>`
        // resolves against /usr/include; the header's own `<mpvqt_version.h>` needs the
        // MpvQt dir directly on the path. MpvAbstractItem is a QQuickFramebufferObject — Qt6Quick
        // (probed above) covers it.
        // /app/... = the Flatpak prefix (10.0): mpvqt + libmpv are built into the app there.
        for inc in ["/usr/include", "/usr/include/MpvQt", "/app/include", "/app/include/MpvQt"] {
            build.include(inc);
        }
        build.compile("tinhatvideobridge");
        println!("cargo:rustc-link-lib=KF6ColorScheme");
        println!("cargo:rustc-link-lib=KF6ConfigCore");
        println!("cargo:rustc-link-lib=KF6ConfigGui");
        // mpvqt: link libMpvQt.so (no pkg-config). pkg-config "mpv" above already emitted -lmpv.
        println!("cargo:rustc-link-lib=MpvQt");
        println!("cargo:rustc-link-search=native=/usr/lib");
    }

    #[cfg(target_os = "windows")]
    {
        // Windows arm (BUILD_WINDOWS.md §9): no KF6 (theming drops KColorScheme — §7.2),
        // no mpv/MpvQt (watch-party deferred — §5.4); video_bridge.cpp #ifdefs those out.
        //
        // 1) GStreamer via pkgconf (.pc ships in the MSVC devel install; PKG_CONFIG_PATH
        //    must point at <gstreamer>\lib\pkgconfig).
        // gstreamer-video-1.0: GstVideoFrame/GstVideoInfo for the Windows tile bridge
        // (video_bridge.cpp qvsBufferProbe) — stride-correct I420 plane access.
        for pkg in ["gstreamer-1.0", "gstreamer-video-1.0"] {
            let gst = pkg_config::Config::new().probe(pkg).unwrap_or_else(|e| {
                panic!(
                    "{pkg} not found ({e}): install GStreamer 1.28 MSVC *devel* and set \
                     PKG_CONFIG_PATH to its lib\\pkgconfig (and PKG_CONFIG to a pkg-config.exe)"
                )
            });
            for inc in &gst.include_paths {
                build.include(inc);
            }
        }

        // 2) Qt include/lib paths from qmake (NO pkg-config for Qt on Windows MSVC;
        //    cxx-qt-build itself also finds Qt via QMAKE — one variable drives both).
        let qmake = std::env::var("QMAKE")
            .expect("set QMAKE=C:\\Qt\\6.8.x\\msvc2022_64\\bin\\qmake.exe");
        let q = |var: &str| {
            let out = std::process::Command::new(&qmake)
                .args(["-query", var])
                .output()
                .expect("run qmake -query");
            String::from_utf8(out.stdout).expect("qmake output utf8").trim().to_string()
        };
        let qt_headers = q("QT_INSTALL_HEADERS"); // e.g. C:/Qt/6.8.3/msvc2022_64/include
        let qt_libs = q("QT_INSTALL_LIBS"); //       e.g. C:/Qt/6.8.3/msvc2022_64/lib
        build.include(&qt_headers);
        for m in ["QtCore", "QtGui", "QtQml", "QtQuick", "QtWebEngineQuick", "QtMultimedia", "QtQuickControls2"] {
            build.include(format!("{qt_headers}/{m}"));
        }
        // 2b) MpvQt (watch-party) — bare-metal port: source-built MSVC MpvQt in the
        //     Kirigami prefix (C:\kde\install, override via TINHAT_MPVQT_PREFIX). When
        //     present, video_bridge.cpp registers the REAL MpvAbstractItem instead of
        //     the QML stub and forces the OpenGL RHI (mpvqt is GL-only). When absent,
        //     the build still succeeds — stub behavior, watch-party surface black.
        let mpvqt_prefix = std::env::var("TINHAT_MPVQT_PREFIX")
            .unwrap_or_else(|_| "C:\\kde\\install".to_string());
        let mpvqt_lib = std::path::Path::new(&mpvqt_prefix).join("lib").join("MpvQt.lib");
        let have_mpvqt = mpvqt_lib.exists();
        println!("cargo:rerun-if-env-changed=TINHAT_MPVQT_PREFIX");
        if have_mpvqt {
            build.include(format!("{mpvqt_prefix}\\include"));
            // mpvqt_export.h does `#include <mpvqt_version.h>` (angle brackets),
            // so the MpvQt subdir itself must be an include root too.
            build.include(format!("{mpvqt_prefix}\\include\\MpvQt"));
            // mpvcontroller.h includes <mpv/client.h> — the shinchiro libmpv
            // dev package headers (override via TINHAT_LIBMPV_PREFIX).
            let libmpv_prefix = std::env::var("TINHAT_LIBMPV_PREFIX")
                .unwrap_or_else(|_| "C:\\kde\\dist\\mpv-dev".to_string());
            build.include(format!("{libmpv_prefix}\\include"));
            println!("cargo:rerun-if-env-changed=TINHAT_LIBMPV_PREFIX");
            build.define("TINHAT_HAVE_MPVQT", None);
            println!("cargo:rustc-link-search=native={mpvqt_prefix}\\lib");
            println!("cargo:rustc-link-lib=MpvQt");
            println!("cargo:warning=MpvQt found at {mpvqt_prefix} — watch-party surface ENABLED (OpenGL RHI)");
        } else {
            println!("cargo:warning=MpvQt not found at {mpvqt_prefix} — watch-party stays stubbed");
        }

        // Windows-only shim: the QVideoSink display-tail prototype (TINHAT_QVS_TEST) —
        // the future primary video path (appsink -> QVideoFrame -> QVideoSink).
        build.file("src/qvideo_bridge.cpp");
        build.flag_if_supported("/EHsc");
        // Qt headers hard-error on MSVC without a truthful __cplusplus macro
        // (qcompilerdetection.h C1189). qmake/CxxQtBuilder pass these themselves;
        // our manual cc::Build must too. /permissive- matches qmake's default strictness.
        build.flag_if_supported("/Zc:__cplusplus");
        build.flag_if_supported("/permissive-");
        build.compile("tinhatvideobridge");

        // 3) Link the Qt libs the shims reference (cxx-qt already links Core/Gui/Qml).
        println!("cargo:rustc-link-search=native={qt_libs}");
        for l in ["Qt6Quick", "Qt6WebEngineQuick", "Qt6WebEngineCore", "Qt6Multimedia", "Qt6QuickControls2"] {
            println!("cargo:rustc-link-lib={l}");
        }
        // Dark DWM title bars (tinhat_dark_titlebars).
        println!("cargo:rustc-link-lib=dwmapi");
        println!("cargo:rerun-if-changed=src/qvideo_bridge.cpp");

        // 4) Exe icon + VERSIONINFO (polish §8.1): taskbar/Explorer/shortcut identity.
        //    tinhat.ico = the Tin Hat mark (the brand kit, 2026-09-16; packaging/icons has the PNG set).
        {
            let mut res = winresource::WindowsResource::new();
            res.set_icon("tinhat.ico");
            res.set("ProductName", "Tin Hat");
            res.set("FileDescription", "Tin Hat");
            if let Err(e) = res.compile() {
                println!("cargo:warning=exe icon resource failed: {e}");
            }
        }
        println!("cargo:rerun-if-changed=tinhat.ico");
    }

    println!("cargo:rerun-if-changed=src/video_bridge.cpp");
    println!("cargo:rerun-if-changed=src/webengine_bridge.cpp");
    println!("cargo:rerun-if-changed=src/clipboard_bridge.cpp");
    println!("cargo:rerun-if-changed=src/theme_bridge.cpp");
}
