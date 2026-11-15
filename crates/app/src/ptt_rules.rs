//! Push to talk's key rules (launch-gate 2026-11-07; shared vectors testvectors/ptt-key.json = the web's pttChord.ts). A
//! modifier on its own (Ctrl by default) doesn't start a hold while a text field has focus: there it is the start of a
//! shortcut, and Ctrl+C must transmit nothing. Any other key pressed during a modifier's hold ends it. A non-modifier
//! binding (an F-key) works everywhere: you can type while holding it.

fn is_modifier(key: &str) -> bool {
    matches!(key, "Ctrl" | "Alt" | "Shift" | "Meta" | "Super")
}

pub(crate) fn press_starts_hold(key: &str, text_focus: bool) -> bool {
    !(text_focus && is_modifier(key))
}

/// 4-1 (2026-11-14): the press rule with the whole event — no OTHER modifier held (Shift+F10 is the menu key, not an F10
/// hold; Shift+Ctrl is a chord) and nothing handled the press already (the web's defaultPrevented; the desktop's shortcuts
/// take their key before any press arrives). = the web's pressStarts; vectors `pressExact`.
pub(crate) fn press_starts(key: &str, other_mods: bool, handled: bool, text_focus: bool) -> bool {
    !other_mods && !handled && press_starts_hold(key, text_focus)
}

/// 4-2 (2026-11-14): no press starts a hold while a dialog, menu or other popup owns the keyboard focus (a Ctrl in the
/// "Send as text?" dialog or a keyboard menu transmitted). = the web's pressStartsIn; vectors `pressPopup`.
pub(crate) fn press_starts_in(key: &str, popup: bool, other_mods: bool, text_focus: bool) -> bool {
    !popup && press_starts(key, other_mods, false, text_focus)
}

/// `held` is the key holding now ("" = none).
pub(crate) fn chord_ends_hold(held: &str, pressed_is_ptt: bool, repeat: bool) -> bool {
    is_modifier(held) && !pressed_is_ptt && !repeat
}

/// BRB privacy (2026-11-13): whether a key press/release leaves me speaking. While I am away (BRB) a press never opens
/// the mic: the default key is Ctrl, so any Ctrl outside a text field used to transmit while I believed I was away.
pub(crate) fn hold_speaks(down: bool, away: bool) -> bool {
    down && !away
}

/// A22.1 (2026-11-13): the "you're muted" bubble under push to talk. A modifier on its own does nothing while a text
/// field has focus, so its hint says so (= the web's mutedHint; the Settings text already explained it).
pub(crate) fn muted_hint(key: &str) -> String {
    if is_modifier(key) {
        format!("You're muted — hold {key} to talk (click outside the message box first)")
    } else {
        format!("You're muted — hold {key} to talk")
    }
}

/// P15 (2026-11-13): how long a release keeps the mic open, so the last word isn't cut (no setting). A press in that
/// time continues the hold. While I am away (BRB ends a hold) the release is immediate.
pub(crate) const RELEASE_TAIL_MS: u64 = 250;
pub(crate) fn release_tail_ms(away: bool) -> u64 {
    if away { 0 } else { RELEASE_TAIL_MS }
}

/// The mic's wanted state with push to talk in the picture. With push to talk ON the mic is muted unless the key is held
/// and I am not away, whatever the mute button was left on (you cannot forget yourself live). With it OFF the mute button
/// decides: BRB mutes through it, and someone who unmutes by hand while away owns that choice.
pub(crate) fn target_muted(ptt_on: bool, holding: bool, manual_muted: bool, away: bool) -> bool {
    if ptt_on {
        away || !holding
    } else {
        manual_muted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const V: &str = include_str!("testvectors/ptt-key.json");

    #[test]
    fn the_shared_vectors() {
        let v: serde_json::Value = serde_json::from_str(V).unwrap();
        for c in v["press"].as_array().unwrap() {
            assert_eq!(press_starts_hold(c["key"].as_str().unwrap(), c["textFocus"].as_bool().unwrap()), c["starts"].as_bool().unwrap(), "{c}");
        }
        for c in v["chord"].as_array().unwrap() {
            assert_eq!(chord_ends_hold(c["held"].as_str().unwrap(), c["pressedIsPtt"].as_bool().unwrap(), c["repeat"].as_bool().unwrap()), c["ends"].as_bool().unwrap(), "{c}");
        }
        let b = |c: &serde_json::Value, k: &str| c[k].as_bool().unwrap();
        let pop = v["pressPopup"].as_array().unwrap();
        assert_eq!(pop.len(), 6);
        for c in pop {
            assert_eq!(press_starts_in(c["key"].as_str().unwrap(), b(c, "popup"), b(c, "otherMods"), b(c, "textFocus")), b(c, "starts"), "{c}");
        }
        let exact = v["pressExact"].as_array().unwrap();
        assert_eq!(exact.len(), 10);
        for c in exact {
            assert_eq!(press_starts(c["key"].as_str().unwrap(), b(c, "otherMods"), b(c, "handled"), b(c, "textFocus")), b(c, "starts"), "{c}");
        }
        let hold = v["hold"].as_array().unwrap();
        assert!(!hold.is_empty());
        for c in hold {
            assert_eq!(hold_speaks(b(c, "down"), b(c, "away")), b(c, "speaks"), "{c}");
        }
        assert_eq!(v["tailMs"].as_u64(), Some(RELEASE_TAIL_MS));
        for c in v["tail"].as_array().unwrap() {
            assert_eq!(release_tail_ms(b(c, "away")), c["ms"].as_u64().unwrap(), "{c}");
        }
        let mic = v["mic"].as_array().unwrap();
        assert_eq!(mic.len(), 16);
        for c in mic {
            assert_eq!(target_muted(b(c, "pttOn"), b(c, "holding"), b(c, "manualMuted"), b(c, "away")), b(c, "muted"), "{c}");
        }
    }

    /// A22.1: the hint's words (the web's pttChord.test runs the same three).
    #[test]
    fn the_muted_hint() {
        assert_eq!(muted_hint("Ctrl"), "You're muted — hold Ctrl to talk (click outside the message box first)");
        assert_eq!(muted_hint("F13"), "You're muted — hold F13 to talk");
        assert_eq!(muted_hint("Alt"), "You're muted — hold Alt to talk (click outside the message box first)");
        assert!(include_str!("../qml/CallIsland.qml").contains("island.app.callctl.pttHint("));
    }

    /// P15: a release mutes only after the tail, a press in the tail cancels it, and BRB's release is immediate.
    #[test]
    fn the_release_tail_is_wired() {
        let cc = include_str!("call_ctl.rs");
        let body = &cc[cc.find("pub fn ptt_hold(mut self").unwrap()..];
        let body = &body[..body.find("\n    }\n").unwrap()];
        assert!(body.contains("crate::ptt_rules::release_tail_ms(away)"), "the release asks the rule");
        assert!(body.contains("PTT_TAIL_GEN") && body.contains("fetch_add(1"), "every press/release cancels a pending tail");
        assert!(body.contains("tokio::time::sleep(std::time::Duration::from_millis(tail))"), "the tail");
    }

    /// BRB ends a push-to-talk hold on its rising edge, before it records what it muted (AppCore's AFK edge): a hold
    /// carried into BRB was otherwise un-muted on the way back with the key long released.
    #[test]
    fn brb_ends_the_hold() {
        let core = include_str!("../qml/AppCore.qml");
        let edge = core.find("_afkMutedMic = !callctl.micMuted").expect("the AFK edge");
        let end = core[..edge].rfind("if (callctl.pttSpeaking) callctl.pttHold(false)").expect("BRB ends the hold");
        assert!(edge - end < 600, "right before the edge records the mic");
    }

    #[test]
    fn the_vectors_are_the_webs() {
        let Ok(dir) = std::env::var("TINHAT_WEB_DIR") else { return };
        let web = std::fs::read_to_string(format!("{dir}/src/features/calls/testvectors/ptt-key.json")).unwrap();
        assert_eq!(web, V, "byte-identical with the web's copy");
    }

    /// 4-1: the desktop passes the press's OTHER modifiers (ptt_bridge.cpp) and Main.qml asks press_starts with them, so
    /// Shift+F10 (the menu key) with an F10 push-to-talk key opens the menu only.
    #[test]
    fn a_modified_key_never_starts_a_hold() {
        let cpp = include_str!("ptt_bridge.cpp");
        assert!(cpp.contains("otherModifiers(key)") && cpp.contains("tinhatPttKeyEvent(QVariant,QVariant,QVariant)"));
        let main = include_str!("../qml/Main.qml");
        assert!(main.contains("function tinhatPttKeyEvent(key, down, otherMods)") && main.contains("callctl.pttPressStartsExact(name, otherMods === true,"));
    }


    /// 4-2: Main.qml asks with whether a popup (anything in the window's overlay: dialogs, menus) owns the focus.
    #[test]
    fn a_popup_with_the_focus_starts_no_hold() {
        let main = include_str!("../qml/Main.qml");
        assert!(main.contains("function _pttPopupFocus()") && main.contains("root._pttTextFocus(), root._pttPopupFocus())"));
        let cc = include_str!("call_ctl.rs");
        assert!(cc.contains("crate::ptt_rules::press_starts_in(&key.to_string(), popup, other_mods, text_focus)"));
    }

}
