//! Pin and full screen follow a blip (D02.3, 2026-11-13; shared vectors testvectors/tilecarry.json = the web's
//! tilecarry.ts). When the pinned or full-screen tile leaves the call, QML holds {participant, kind, until} for the
//! leave-chime grace; a tile of the same participant (device included) and kind arriving in that time is pinned / made
//! full screen again, so a sharer's blip no longer drops the viewer's pin and full screen.
use serde_json::Value;

pub(crate) const CARRY_GRACE_MS: u64 = crate::persontiles::PAD_LEAVE_GRACE_MS;

fn kind_of(t: &Value) -> &str {
    t.get("kind").and_then(Value::as_str).unwrap_or("video")
}

/// The tile to carry the pin / full screen to, or None.
pub(crate) fn carry_target(participant: &str, kind: &str, until: u64, tiles: &[Value], now: u64) -> Option<String> {
    if participant.is_empty() || now >= until {
        return None;
    }
    tiles
        .iter()
        .find(|t| t.get("participant").and_then(Value::as_str) == Some(participant) && kind_of(t) == kind)
        .and_then(|t| t.get("id").and_then(Value::as_str))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    const V: &str = include_str!("testvectors/tilecarry.json");

    #[test]
    fn the_shared_vectors() {
        let v: Value = serde_json::from_str(V).unwrap();
        assert_eq!(v["graceMs"].as_u64(), Some(CARRY_GRACE_MS));
        let cases = v["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 8);
        for c in cases {
            let h = &c["held"];
            let got = if h.is_null() {
                None
            } else {
                carry_target(h["participant"].as_str().unwrap(), h["kind"].as_str().unwrap(), h["until"].as_u64().unwrap(), c["tiles"].as_array().unwrap(), c["now"].as_u64().unwrap())
            };
            assert_eq!(got.as_deref(), c["want"].as_str(), "{}", c["name"]);
        }
    }

    #[test]
    fn the_vectors_are_the_webs() {
        let Ok(dir) = std::env::var("TINHAT_WEB_DIR") else { return };
        let web = std::fs::read_to_string(format!("{dir}/src/features/calls/testvectors/tilecarry.json")).unwrap();
        assert_eq!(web, V, "byte-identical with the web's copy");
    }

    /// The call UI holds the pin and the full screen through a blip and asks this rule (callctl.carryTile).
    #[test]
    fn the_ui_carries() {
        let core = include_str!("../qml/AppCore.qml");
        assert!(core.contains("core._pinHeld = core._heldOf(core.pinnedTile)") && core.contains("callctl.carryTile("), "the pin");
        // (review) hold before carry (a same-update replacement), and a pin the person changed meanwhile wins
        assert!(core.find("core._pinHeld = core._heldOf(core.pinnedTile)").unwrap() < core.find("var to = mine ? core._carryTo(core._pinHeld)").unwrap());
        assert!(core.contains("core.pinnedTile === core._pinHeld.from"));
        // (review r2) a stale hold goes when another tile is pinned; the person's own unpin cancels the carry
        assert!(core.contains("if (core._pinHeld && core.pinnedTile.length > 0 && core.pinnedTile !== core._pinHeld.from) core._pinHeld = null"));
        assert!(core.contains("if (core.pinnedTile.length === 0 && !core._sysUnpin) core._pinHeld = null"));
        let main = include_str!("../qml/Main.qml");
        assert!(main.contains("core._fsHeld = core._heldOf(core.shareFullscreen)") && main.contains("fsCarryTimer.restart()"), "the full screen");
        assert!(main.contains("if (core._fsHeld) { fsCarryTimer.stop(); core._fsHeld = null }"), "(review) the same tile back ends the hold");
    }

    /// 5-8: the missing pin is cleared when its hold is made (before the PiP early return), so an expired hold has
    /// nothing to re-arm from: someone who really left doesn't regain the pin when they come back later.
    #[test]
    fn a_real_leave_never_re_arms_the_pin() {
        let core = include_str!("../qml/AppCore.qml");
        let h = core.find("core._pinHeld = core._heldOf(core.pinnedTile)").unwrap();
        let unpin = core[h..].find("core._sysUnpin = true\n            core.pinnedTile = \"\"").map(|x| x + h).expect("the pin is cleared with the hold");
        let early = core.find("if (keys.length === 0) return").unwrap();
        assert!(unpin < early, "before the PiP early return");
    }


    /// 12.1.17 room-link gate: its paddrive steps (open-room, open-thread, jump-event, room-state, find-text, find-link) sit inside the lab fence
    /// (login.labDriveHooks(): cfg tinhat_env_selftests + TINHAT_LABDRIVE_PTT, both refused in a release), so a release
    /// binary answers "refused" to them.
    #[test]
    fn the_roomlink_gate_steps_are_fenced() {
        let main = include_str!("../qml/Main.qml");
        let fence = main.find("else if ((s.do === \"row\" || s.do === \"find-action\"").expect("the lab fence");
        let end = fence + main[fence..].find("&& !login.labDriveHooks()) { r = \"refused\" }").expect("its refusal");
        for step in ["open-room", "open-thread", "jump-event", "room-state", "find-text", "find-link"] {
            let q = format!("s.do === \"{step}\"");
            assert!(main[fence..end].contains(&q), "{step} is inside the lab fence");
            let first = main.find(&format!("else if ({q})")).unwrap_or_else(|| panic!("{step} is handled"));
            assert!(first > end, "{step} is handled only after the fence");
        }
    }

    /// fix1216 round 2: the lab drive steps exist for the gate (pin, pin-state, force-drop, chimes) behind the lab fence,
    /// and the drop hook's env name is on the release-binary check's list (a lab binary is refused for it).
    #[test]
    fn the_lab_steps_are_fenced() {
        let main = include_str!("../qml/Main.qml");
        assert!(main.contains("|| s.do === \"force-drop\") && !login.labDriveHooks()) { r = \"refused\" }"));
        assert!(main.contains("else if (s.do === \"pin\") {") && main.contains("console.log(\"[paddrive] chime \" + kind"));
        let cc = include_str!("call_ctl.rs");
        let f = &cc[cc.find("pub fn lab_force_drop(&self) -> bool {").unwrap()..];
        assert!(f[..f.find("pub fn carry_tile(").unwrap()].contains("#[cfg(not(tinhat_env_selftests))]"));
        let st = include_str!("../../streamer/src/call.rs");
        let g = st.find("pub fn lab_force_terminal_drop(&self)").unwrap();
        assert!(st[..g].ends_with("    #[cfg(tinhat_env_selftests)]\n    "));
        let rb = include_str!("../../../scripts/release-binary-check.sh");
        assert!(rb.contains(" TINHAT_LABDRIVE_DROP "));
    }

}
