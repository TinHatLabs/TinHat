//! The words the app shows, pinned across every QML and Rust source file.
//! Mirrored by the web's src/app/wording.test.ts; keep the two lists the same.
#![cfg(test)]

use std::path::{Path, PathBuf};

fn files(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).expect("read_dir").flatten() {
        let p = e.path();
        if p.is_dir() {
            files(&p, exts, out);
        } else if p.extension().and_then(|x| x.to_str()).is_some_and(|x| exts.contains(&x)) {
            out.push(p);
        }
    }
}

/// Code lines of every QML + Rust source (whole-line comments are skipped: they may name old words on purpose).
/// This file is left out: it names the old words to forbid them.
pub(crate) fn code_lines() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut fs = Vec::new();
    files(&root.join("qml"), &["qml", "js"], &mut fs);
    files(&root.join("src"), &["rs"], &mut fs);
    let mut out = Vec::new();
    for f in fs {
        if f.ends_with("wording.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        for (i, l) in text.lines().enumerate() {
            let t = l.trim_start();
            if t.starts_with("//") || t.starts_with('*') || t.starts_with("/*") {
                continue;
            }
            out.push((format!("{}:{}", f.strip_prefix(root).unwrap_or(&f).display(), i + 1), l.to_string()));
        }
    }
    out
}

#[test]
fn one_name_for_the_key_security_key_never_recovery_key() {
    // P16: "Security Key" everywhere (toasts, Settings, screen-reader labels).
    let hits: Vec<String> = code_lines()
        .into_iter()
        .filter(|(_, l)| l.to_lowercase().contains("recovery key"))
        .map(|(at, _)| at)
        .collect();
    assert!(hits.is_empty(), "\"recovery key\" shown at: {hits:?}");
}

#[test]
fn update_strip_has_later_and_keeps_whats_new() {
    // P17: "Later" hides the strip until a newer version (the web's updateLater.ts: the same rule and pref name);
    // What's new stays on the Restart-now strip.
    let main = include_str!("../qml/Main.qml");
    assert!(main.contains(r#"property string updateLaterFor: settings ? settings.uiPref("update-later-for") : """#), "the stored version");
    assert!(
        main.contains("&& (mUpdateDownloading || updateLaterFor.length === 0 || updateLaterFor !== mLatestVersion)"),
        "hidden only for the version put off, never mid-download"
    );
    assert!(main.contains("visible: root.updateBannerUp\n"), "the strip follows the rule");
    assert!(main.contains("|| root.updateBannerUp))"), "and so does the banner host");
    assert!(main.contains(r#"text: "Later""#) && main.contains("onTriggered: root.hideUpdateUntilNext()"), "the Later action");
    assert!(
        main.contains("visible: (root.mUpdateAvailable || root.mUpdateReady) && root.mReleaseNotes.length > 0"),
        "What's new after the download"
    );
}

#[test]
fn update_check_repeats_while_the_app_stays_open() {
    // A32.0: not only once per session; never mid-download or with a restart pending (12.1.15's updater safety).
    let core = include_str!("../qml/AppCore.qml");
    let at = core.find("id: updateRecheckTimer").expect("the re-check timer");
    let t = &core[at..at + 400];
    assert!(t.contains("interval: 4 * 60 * 60 * 1000") && t.contains("repeat: true"), "every 4 hours");
    assert!(t.contains("onTriggered: if (!core.mUpdateDownloading && !core.mUpdateReady) updater.checkForUpdates()"));
}

#[test]
fn room_kick_and_ban_ask_for_a_reason() {
    // A30.2: the room confirms ask for a reason like the space ones (web RightPanel.tsx: the same).
    let rp = include_str!("../qml/RightPanel.qml");
    assert!(rp.contains(r#"askReason: panel.actorMembership !== "invite""#), "kick (not a withdrawn invite)");
    assert!(rp.contains("onConfirmed: panel.kickUser(panel.actorMxid, kickConfirm.reason)"));
    assert!(rp.contains("onConfirmed: panel.banUser(panel.actorMxid, banConfirm.reason)"));
    let ban = &rp[rp.find("id: banConfirm").expect("ban confirm")..];
    assert!(ban[..ban.len().min(400)].contains("askReason: true"));
}

#[test]
fn admins_can_make_admins_after_a_confirm() {
    // P13: up to my own level; my own level asks first (web roleChoices.ts + its test: the same rule and words).
    let rp = include_str!("../qml/RightPanel.qml");
    assert!(rp.contains("visible: modelData.v <= panel.myPower"), "an Admin sees Admin");
    assert!(rp.contains("if (modelData.v >= panel.myPower && modelData.v !== panel.actorPower) {"), "my own level confirms");
    assert!(rp.contains(r#"dialogTitle: "Make " + panel.actorName + (/^[AEIOU]/.test(role) ? " an " : " a ") + role + "?""#));
    assert!(rp.contains(r#"confirmText: "Make " + role"#));
    assert!(rp.contains(r#"message: "They'll have the same power as you, and you won't be able to undo it.""#));
    assert!(rp.contains("onConfirmed: panel.setMemberPower(roleConfirm.target, roleConfirm.level)"));
}

#[test]
fn grouped_follow_ups_show_their_clock_on_hover_only() {
    // P19 (web lib/stamp.ts clockQuiet + its test: the same rule).
    let cb = include_str!("../qml/ChatBubble.qml");
    assert!(cb.contains("readonly property bool clockQuiet: grouped && !pending && !failed && !pinned && !edited"));
    assert!(cb.contains(
        "opacity: bubble.failed ? 1.0 : ((bubble.clockQuiet && !bubbleHover.hovered && !bubble.activeFocus && !bubble.kbCurrent) ? 0.0 : 0.7)"
    ));
}

#[test]
fn no_diagnostics_switch_create_report_works_straight_away() {
    // P18 (web settings/panes/diagnostics.test.ts: the same).
    let sw = include_str!("../qml/SettingsWindow.qml");
    let dg = include_str!("diagnostics.rs");
    let ah = include_str!("../qml/ActionsHost.qml");
    assert!(!sw.contains("diagnostics.setOptIn") && !sw.contains(r#"label: "Diagnostic reports""#), "no switch row");
    assert!(sw.contains(r#"ThButton { app: win.app; text: "Create report"; onClicked: win.app.diagnostics.createReport() },"#));
    assert!(!dg.contains("opted_in()") && !dg.contains("diagnostics-opt-in"), "no opt-in gate");
    assert!(ah.contains(r#""prefs.diagnostics": true,"#), "the palette's Create diagnostic report is on");
}

#[test]
fn unknown_slash_commands_ask_before_going_out_as_text() {
    // P21 (web lib/markdown.ts unknownSlash/unknownSlashWords + lib/slash.test.ts: the same list, rule and words).
    let cv = include_str!("../qml/ConversationView.qml");
    assert!(cv.contains(r#"var m = /^\/([A-Za-z][\w-]*)(?:\s|$)/.exec(String(t).replace(/^\s+/, ""))"#));
    assert!(cv.contains(r#"return ["me", "shrug", "tableflip", "unflip", "lenny"].indexOf(m[1].toLowerCase()) !== -1 ? "" : m[1]"#));
    assert!(cv.contains("if (hasText && asText !== true && conv.editToId.length === 0 && conv._unknownSlash(t).length > 0) {"));
    assert!(cv.contains(r#"dialogTitle: "Unknown command — send as text?""#));
    assert!(cv.contains(r#"confirmText: "Send as text""#));
    assert!(cv.contains(
        r#"message: "There's no /" + cmd + " command. The ones you can use: /me, /shrug, /tableflip, /unflip, /lenny.""#
    ));
    // 4-4: confirm sends only what the question showed (tools/send-as-text-test.mjs)
    assert!(cv.contains("if (slashConfirm.asked && conv.draftSame(slashConfirm.asked, conv._sendSnap())) conv.sendComposer(true)"));
    // "Send as text" sends exactly what was typed ("/shrug-test" must not become "-test ¯\_(ツ)_/¯")
    assert!(cv.contains("var expand = asText === true ? function (s) { return String(s).trim() } : conv._expandSlash"));
    let send = &cv[cv.find("function sendComposer(asText) {").unwrap()..cv.find("function cancelTarget() {").unwrap()];
    assert!(!send.contains("conv._expandSlash("), "every send path goes through expand");
}

#[test]
fn join_fail_words_follow_the_shared_vectors() {
    // A36.1: the call service's raw errors never reach "Couldn't join the call — …" (web joinfail.test.ts: the same file).
    let v: serde_json::Value = serde_json::from_str(include_str!("testvectors/join-fail.json")).expect("json");
    let cases = v["cases"].as_array().expect("cases");
    assert!(cases.len() >= 15);
    for c in cases {
        let raw = c["raw"].as_str().unwrap();
        let got = crate::call_ctl::join_fail_reason(raw);
        assert_eq!(got, c["want"].as_str().unwrap(), "{raw}");
        assert!(!got.contains("lk-jwt") && !got.contains("<html>") && !got.contains("url/jwt"), "{raw}");
    }
}

/// The byte copy equals the web's (`TINHAT_WEB_DIR` = a tinhat-web tree, else `$HOME/tinhat-web`; skipped when neither has it).
#[test]
fn join_fail_vectors_are_the_webs() {
    let dir = std::env::var_os("TINHAT_WEB_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("tinhat-web")));
    let Some(dir) = dir else { return };
    let path = dir.join("src/features/calls/testvectors/join-fail.json");
    match std::fs::read_to_string(&path) {
        Ok(theirs) => assert!(include_str!("testvectors/join-fail.json") == theirs, "join-fail.json differs from {}", path.display()),
        Err(_) => eprintln!("SKIPPED: no web vectors at {}", path.display()),
    }
}

#[test]
fn locked_messages_that_ask_to_verify_offer_it() {
    // A28.3 (web lib/utd.ts utdOffersVerify + utd.test.ts: the same words, the same two causes).
    use crate::utd::UtdCause::*;
    let offering: Vec<_> = [BeforeFirstSignIn, BeforeThisDevice, BeforeThisDeviceVerify, BeforeJoined, WithheldUnverified, Withheld, Waiting]
        .into_iter()
        .filter(|c| c.text().contains("Verify this device"))
        .map(|c| c.key())
        .collect();
    assert_eq!(offering, ["beforeThisDeviceVerify", "withheldUnverified"]);
    let cb = include_str!("../qml/ChatBubble.qml");
    assert!(cb.contains(r#"visible: bubble.undecryptable && lockedText.text.indexOf("Verify this device") !== -1"#));
    assert!(cb.contains(r#"text: "Verify this device…""#) && cb.contains("onClicked: bubble.app.openVerifySheet()"));
    assert!(include_str!("../qml/Main.qml").contains("function openVerifySheet() { verifySheet.open() }"));
}

#[test]
fn every_settings_index_row_has_its_anchor() {
    // A24.0: search can land on every row of the shared index (web settingsAnchors.test.ts: the same check on its sources).
    let idx: serde_json::Value = serde_json::from_str(include_str!("settings_index.json")).expect("json");
    let qml: String = code_lines().into_iter().filter(|(at, _)| at.contains(".qml:")).map(|(_, l)| l + "\n").collect();
    let missing: Vec<String> = idx["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_string())
        .filter(|id| !qml.contains(&format!("\"{id}\"")))
        .collect();
    assert!(missing.is_empty(), "no anchor for {missing:?}");
    for id in ["appearance.style", "appearance.layout", "appearance.buttons", "appearance.gif-autoplay", "appearance.gpu-saver"] {
        assert!(idx["entries"].as_array().unwrap().iter().any(|e| e["id"] == id), "{id} in the index");
    }
}

#[test]
fn the_devices_list_failure_has_a_way_out() {
    // D10.8 (web settings/panes/devicesRetry.test.ts: the same words and button).
    let sw = include_str!("../qml/SettingsWindow.qml");
    assert!(!sw.contains("Close and reopen Settings to try again."), "no dead end");
    assert!(sw.contains(r#"label: win.sBusy ? "Loading your devices…" : "Couldn't load your devices""#));
    assert!(sw.contains(r#"description: win.sBusy ? "" : "Check your connection, then try again.""#));
    let at = sw.find(r#"text: "Try again""#).expect("Try again");
    assert!(sw[at..at + 200].contains("onClicked: win.app.settings.refresh()"));
}

#[test]
fn the_devices_list_says_device() {
    // A36.2 (web devicesRetry.test.ts: no "session" in its devices pane either).
    let sw = include_str!("../qml/SettingsWindow.qml");
    assert!(sw.contains(r#"Controls.ToolTip.text: "Device " + devRow.modelData.id"#));
    assert!(!sw.contains(r#""Session " + devRow"#) && !sw.contains("your sessions"));
}

#[test]
fn a_failed_role_change_says_role() {
    // A30.5 (web roomModeration.test.ts: the same words).
    let room = include_str!("room.rs");
    assert!(room.contains(r#"mod_err("change their role", &e)"#));
    assert!(!room.contains(r#"mod_err("change that power level""#));
}

#[test]
fn the_security_key_field_can_be_shown() {
    // A28.2 (web verifyKeyShow.test.ts: the same box and words).
    let v = include_str!("../qml/VerifyDeviceDialog.qml");
    assert!(v.contains("echoMode: showKeyBox.checked ? TextInput.Normal : TextInput.Password"));
    assert!(v.contains(r#"text: "Show the key""#));
    assert!(v.contains(r#"function clearForm() { secKeyField.text = ""; showKeyBox.checked = false;"#), "masked again next time");
}

#[test]
fn soundboard_upload_failures_follow_the_shared_vectors() {
    // A20.2 (web media/soundboard/uploadWords.test.ts: the same file).
    let v: serde_json::Value = serde_json::from_str(include_str!("testvectors/soundboard-upload.json")).expect("json");
    let cases = v["cases"].as_array().unwrap();
    assert!(cases.len() >= 12);
    for c in cases {
        let (kind, status) = (c["kind"].as_str().unwrap(), c["status"].as_u64().unwrap() as u16);
        assert_eq!(crate::soundboard::upload_fail_words(kind, status), c["want"].as_str().unwrap(), "{kind} {status}");
    }
    let sb = include_str!("soundboard.rs");
    assert!(sb.contains("o.as_mut().set_error(QString::from(&upload_fail_words(kind, status)));"));
    assert!(!sb.contains(r#"format!("Upload failed: {e}")"#));
}

#[test]
fn soundboard_upload_vectors_are_the_webs() {
    let dir = std::env::var_os("TINHAT_WEB_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("tinhat-web")));
    let Some(dir) = dir else { return };
    let path = dir.join("src/features/media/soundboard/testvectors/soundboard-upload.json");
    match std::fs::read_to_string(&path) {
        Ok(theirs) => assert!(include_str!("testvectors/soundboard-upload.json") == theirs, "differs from {}", path.display()),
        Err(_) => eprintln!("SKIPPED: no web vectors at {}", path.display()),
    }
}

#[test]
fn the_edit_layout_words_follow_the_current_key() {
    // A17.2 (web editmode/hints.ts + hints.test.ts: the same words).
    let ls = include_str!("../qml/LayoutSection.qml");
    let tb = include_str!("../qml/EditToolbar.qml");
    assert!(ls.contains(r#"sec.app.actions.keysFor("layout.edit")"#) && tb.contains(r#"tb.em.app.actions.keysFor("layout.edit")"#));
    assert!(ls.contains(r#"? "Move, resize and add islands. " + editKeys[0] + " or right-clicking a gap between islands opens it.""#));
    assert!(ls.contains(r#": "Move, resize and add islands. Right-clicking a gap between islands opens it.""#));
    assert!(tb.contains(r#"tip: tb.em.readOnly ? "" : ((tb.em.dirty ? "Save and close" : "Close") + (editKeys.length > 0 ? " (" + editKeys[0] + ")" : ""))"#));
    assert!(!ls.contains("Ctrl+E") && !tb.contains("(Ctrl+E)"), "no hard-coded key");
}

#[test]
fn moderators_can_delete_others_messages() {
    // A30.0 (web chat/lib/deleteRule.ts + its test: the same rule).
    let cv = include_str!("../qml/ConversationView.qml");
    assert!(cv.contains(
        "visible: (conv.ctxMsgMine || (!!conv.room && conv.room.canRedactOthers === true)) && !conv.ctxMsgRedacted && !conv.ctxMsgEcho"
    ));
    let room = include_str!("room.rs");
    assert!(room.contains("#[qproperty(bool, can_redact_others)]"));
    assert!(room.contains("o.as_mut().set_can_redact_others(self.poll_can_redact);"));
    assert!(room.contains("mine = pl.user_can_redact_event_of_other(room.own_user_id());"), "the redact level for me");
}

#[test]
fn the_density_row_is_a_named_group() {
    // A25.7: three radio buttons in a group named "Density" (the web's radiogroup), not one radio button.
    let t = include_str!("../qml/ThemeEditorSimple.qml");
    let at = t.find(r#"SectionLabel { app: simple.app; text: "Density" }"#).expect("the Density section");
    let row = &t[at..at + 600];
    assert!(row.contains("Accessible.role: Accessible.Grouping") && row.contains(r#"Accessible.name: "Density""#));
    assert!(!row.contains("Accessible.role: Accessible.RadioButton"));
}

#[test]
fn an_empty_theme_name_is_answered() {
    // A25.6 (web settings/panes/renameTheme.test.ts: the same words).
    let a = include_str!("../qml/AppearancePane.qml");
    assert!(a.contains(
        r#"if (n.length < 1) { pane.app.settingsToast("A theme needs a name — it keeps “" + renameDialog.oldName + "”.", "long"); return }"#
    ));
}

#[test]
fn hiding_or_removing_an_island_says_so() {
    // A17.0 (web editmode/hints.ts + hints.test.ts: the same words).
    let em = include_str!("../qml/EditMode.qml");
    assert!(em.contains(r#"function hide(ref) { if (em.apply({ op: "hide", ref: ref }).ok) em.toast(em.islandName(ref) + " hidden — Undo brings it back.", 3000) }"#));
    assert!(em.contains(r#"em.toast("Button island removed — Undo brings it back.", 3000)"#));
}

#[test]
fn watch_party_tabs_chips_and_suggestions_work_by_keyboard() {
    // A31.3: tabs (PageTab), the chips (toggle buttons) and the suggestions (list items) take focus and Enter.
    let wp = include_str!("../qml/WatchPartyWindow.qml");
    for (role, act) in [
        ("Accessible.role: Accessible.PageTab", "Keys.onReturnPressed: sheet.setTab(modelData.id)"),
        ("Accessible.role: Accessible.Button", "Keys.onReturnPressed: pick()"),
        ("Accessible.role: Accessible.ListItem", "Accessible.onPressAction: pick()"),
    ] {
        assert!(wp.contains(role) && wp.contains(act), "{role}");
    }
    assert!(wp.matches("activeFocusOnTab: true").count() >= 3);
}

#[test]
fn no_jargon_on_screen() {
    // A36.3 (web src/app/wording.test.ts: the same words): "cross-signed", "the homeserver", "Matrix apps".
    let hits: Vec<String> = code_lines()
        .into_iter()
        .filter(|(at, l)| at.contains(".qml:") && l.contains('"') && (l.contains("cross-signed") || l.contains("the homeserver") || l.contains("Matrix apps")))
        .map(|(at, _)| at)
        .collect();
    assert!(hits.is_empty(), "jargon at: {hits:?}");
}

#[test]
fn the_widgets_island_menu_offers_add_widget() {
    // A17.6 (web editmode/hints.test.ts: the same item).
    let el = include_str!("../qml/EditLayer.qml");
    let at = el.find(r#"text: "Add widget…""#).expect("Add widget…");
    let item = &el[at..at + 260];
    assert!(item.contains(r#"visible: el.em.kindOf(islandMenu.ref) === "widgets""#) && item.contains("onTriggered: el.em.addWidget()"));
}
