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

/// `held` is the key holding now ("" = none).
pub(crate) fn chord_ends_hold(held: &str, pressed_is_ptt: bool, repeat: bool) -> bool {
    is_modifier(held) && !pressed_is_ptt && !repeat
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
    }

    #[test]
    fn the_vectors_are_the_webs() {
        let Ok(dir) = std::env::var("TINHAT_WEB_DIR") else { return };
        let web = std::fs::read_to_string(format!("{dir}/src/features/calls/testvectors/ptt-key.json")).unwrap();
        assert_eq!(web, V, "byte-identical with the web's copy");
    }
}
