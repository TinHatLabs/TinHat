//! replace_confirm — a password sign-in (or a new account) while this device still holds a saved session asks first
//! (polish-night, 2026-10-28; the owner's decision). Signing in starts from a clean store, so the saved session, its
//! device and any keys only it holds would go. Tin Hat Web asks with the same words (core/matrix/replaceConfirm.ts).
//! The self-test sign-in (TINHAT_GUI_LOGIN) is not a person and never asks.

/// The question, word for word in both apps (Continue / Back).
pub const TEXT: &str = "This replaces the saved session on this device. Continue?";
pub const CONTINUE: &str = "Continue";
pub const BACK: &str = "Back";

/// Ask before a sign-in? (pure)
pub fn needed(saved_session: bool, self_test: bool) -> bool {
    saved_session && !self_test
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_only_when_a_saved_session_would_be_replaced() {
        assert!(needed(true, false));
        assert!(!needed(false, false));
        assert!(!needed(true, true)); // the self-test hook never asks
        assert!(!needed(false, true));
    }

    #[test]
    fn the_words_are_the_webs() {
        // core/matrix/replaceConfirm.ts has the same three strings (its test checks the same values)
        assert_eq!(TEXT, "This replaces the saved session on this device. Continue?");
        assert_eq!((CONTINUE, BACK), ("Continue", "Back"));
    }
}
