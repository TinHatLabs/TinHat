//! What a kick / Disconnect / Mute says when the person is already gone (guest-leave follow-up, 12.1.1). The menus no
//! longer list someone who left (the roster rule), so these words show in one case: the person leaves (or is kicked by
//! someone else) while the confirmation for "Kick…" / "Disconnect from the call…" is open, then Confirm.
//! Pure. = the web's `src/features/spaces/leftWords.ts` `goneText`; the tests replay its cases.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeftScope {
    Room,
    Space,
    Call,
}

fn gone(m: Option<&str>) -> bool {
    matches!(m, Some("leave") | Some("ban"))
}

/// None = go ahead; else the sentence that replaces the action (`membership` as Matrix spells it, None = unknown):
/// - Room: left / banned → "<name> has already left"
/// - Space: only a joined or invited member can be removed; left / banned → "<name> has already left the space",
///   anything else → "<name> isn't in the space"
/// - Call (Disconnect / Mute act on the call server): `on_call_server` = they still have a tile there → go ahead; else
///   left / banned → "<name> has already left", otherwise "They're not in the call"
pub fn gone_text(scope: LeftScope, name: &str, membership: Option<&str>, on_call_server: bool) -> Option<String> {
    match scope {
        LeftScope::Room => gone(membership).then(|| format!("{name} has already left")),
        LeftScope::Space => match membership {
            Some("join") | Some("invite") => None,
            m if gone(m) => Some(format!("{name} has already left the space")),
            _ => Some(format!("{name} isn't in the space")),
        },
        LeftScope::Call if on_call_server => None,
        LeftScope::Call => Some(if gone(membership) { format!("{name} has already left") } else { "They're not in the call".to_string() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// = the web's leftWords.test.ts CASES, in the same order.
    #[test]
    fn cases_match_the_web() {
        use LeftScope::*;
        let cases: &[(LeftScope, &str, Option<&str>, bool, Option<&str>)] = &[
            (Room, "Ana", Some("leave"), false, Some("Ana has already left")),
            (Room, "Ana", Some("ban"), false, Some("Ana has already left")),
            (Room, "Ana", Some("join"), false, None),
            (Room, "Ana", Some("invite"), false, None),
            (Room, "Ana", None, false, None),
            (Space, "Ana", Some("join"), false, None),
            (Space, "Ana", Some("invite"), false, None),
            (Space, "Ana", Some("leave"), false, Some("Ana has already left the space")),
            (Space, "Ana", Some("ban"), false, Some("Ana has already left the space")),
            (Space, "Ana", Some("knock"), false, Some("Ana isn't in the space")),
            (Space, "Ana", None, false, Some("Ana isn't in the space")),
            (Call, "Ana", Some("join"), true, None),
            (Call, "Ana", Some("leave"), true, None),
            (Call, "Ana", Some("leave"), false, Some("Ana has already left")),
            (Call, "Ana", Some("ban"), false, Some("Ana has already left")),
            (Call, "Ana", Some("join"), false, Some("They're not in the call")),
            (Call, "Ana", None, false, Some("They're not in the call")),
        ];
        for (scope, name, m, on, want) in cases {
            assert_eq!(gone_text(*scope, name, *m, *on).as_deref(), *want, "{scope:?} {m:?} {on}");
        }
    }
}
