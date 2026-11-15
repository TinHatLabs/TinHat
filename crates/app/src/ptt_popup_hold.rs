//! 12.1.16 fix round, review round on top of 4-1 / 4-2 (ptt_rules::press_starts / press_starts_in): AltGr counts as another
//! modifier, and a key hold ends when a dialog, menu or popup takes the keyboard (Main.qml). Real-key twin:
//! scripts/ptt-keys-gate.sh (H1, N1), on xcb.

#[test]
fn altgr_is_another_modifier() {
    let cpp = include_str!("ptt_bridge.cpp");
    assert!(cpp.contains("Qt::MetaModifier | Qt::GroupSwitchModifier);"), "AltGr is read");
    assert!(cpp.contains("case Qt::Key_AltGr: m &= ~(Qt::AltModifier | Qt::GroupSwitchModifier); break;"), "an AltGr binding's own bit");
    assert!(cpp.contains("case Qt::Key_Alt: m &= ~Qt::AltModifier; break;"), "an Alt binding keeps AltGr as another modifier");
}

#[test]
fn a_popup_taking_the_keyboard_ends_a_key_hold() {
    let main = include_str!("../qml/Main.qml");
    let at = main.find("function onActiveFocusItemChanged() {\n            if (root._pttHeldKey !== \"\" && root._pttPopupFocus()) {").expect("the edge");
    let body = &main[at..at + 300];
    assert!(body.contains("root._pttHeldKey = \"\"") && body.contains("callctl.pttHold(false)"), "the ordinary release");
}
