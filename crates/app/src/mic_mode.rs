//! The mic-mode invariant (4-9, launch-gate 2026-11-14; shared vectors testvectors/mic-mode.json = the web's micMode.ts):
//! after ANY mic-mode change the mic ends MUTED unless the person explicitly unmuted it. Turning push to talk OFF while
//! holding its key used to leave the mic open (the setting kept the hold's unmuted state, and the release returned early
//! once push to talk was off); leaving a call during a hold kept the hold for the next call.

/// The push-to-talk setting changed (either way, also from its call-bar button): a hold ends and the mic is muted.
/// -> (speaking, muted)
pub(crate) fn ptt_toggled(_on: bool) -> (bool, bool) {
    (false, true)
}

/// Leaving the call ends a hold; in push to talk the mic is muted (out of it the mute button's state stays).
/// -> (speaking, muted)
pub(crate) fn leave_hold(ptt: bool, muted: bool) -> (bool, bool) {
    (false, ptt || muted)
}

/// 4-9 (review): a mic write (its generation) still applies only while it is the newest one — the writes are separate
/// tasks and can take the call lock out of order.
pub(crate) fn write_is_current(gen: u64, current: u64) -> bool {
    gen == current
}

/// 4-9 (review): what a mic rebuild publishes — its own captured state, unless a newer write came after it was captured
/// (then that write's value: a mode change's mute must not be undone by a rebuild already on its way).
pub(crate) fn rebuild_muted(captured: bool, gen_at_capture: u64, gen_now: u64, newest: bool) -> bool {
    if gen_now != gen_at_capture { newest } else { captured }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ptt_rules::{hold_speaks, target_muted};
    const V: &str = include_str!("testvectors/mic-mode.json");

    #[derive(Default, Clone, Copy)]
    struct Mic {
        ptt: bool,
        speaking: bool,
        muted: bool,
        away: bool,
        afk_muted: bool,
    }
    fn live(m: &Mic) -> bool {
        !(m.muted || (m.ptt && target_muted(true, m.speaking, m.muted, m.away)))
    }
    /// The controller's steps, by the functions it calls (call_ctl.rs enable_push_to_talk / ptt_hold / leave_call / the
    /// join's mute, AppCore's AFK edge). `base` = the 12.1.15 rules of the two steps 4-9 changes (the control).
    fn step(m: &mut Mic, ev: &str, base: bool) {
        match ev {
            "pttOn" | "pttOff" => {
                let on = ev == "pttOn";
                if base {
                    // 12.1.15: target = ptt_target_muted(on, false, manual)
                    m.muted = if on { true } else { m.muted };
                    m.speaking = false;
                } else {
                    (m.speaking, m.muted) = ptt_toggled(on);
                    m.afk_muted = false; // the mode change is the person's: BRB's restore claim goes (AppCore)
                }
                m.ptt = on;
            }
            "holdDown" => {
                if m.ptt && !m.speaking && hold_speaks(true, m.away) {
                    m.speaking = true;
                    m.muted = target_muted(true, true, true, m.away);
                }
            }
            "holdUp" => {
                if m.ptt && m.speaking {
                    m.speaking = false;
                    m.muted = target_muted(true, false, true, m.away);
                }
            }
            "muteToggle" => m.muted = !m.muted,
            "deviceSwitch" => {}
            "leave" => {
                if !base {
                    (m.speaking, m.muted) = leave_hold(m.ptt, m.muted);
                }
            }
            "join" => m.muted = m.muted || (m.ptt && !m.speaking),
            "afkOn" => {
                m.away = true;
                if m.speaking {
                    m.speaking = false;
                    m.muted = true;
                }
                m.afk_muted = !m.muted;
                m.muted = true;
            }
            "afkOff" => {
                m.away = false;
                if m.afk_muted {
                    m.muted = false;
                }
                m.afk_muted = false;
            }
            e => panic!("unknown event {e}"),
        }
    }
    fn run(sc: &serde_json::Value, base: bool) -> bool {
        let mut m = Mic { muted: true, ..Default::default() };
        sc["steps"].as_array().unwrap().iter().all(|st| {
            step(&mut m, st["event"].as_str().unwrap(), base);
            live(&m) == st["live"].as_bool().unwrap()
        })
    }

    /// 4-9: the matrix — every scenario holds with today's rules; the 12.1.15 rules fail exactly the hot-mic rows.
    #[test]
    fn the_mic_mode_matrix() {
        let v: serde_json::Value = serde_json::from_str(V).unwrap();
        let sc = v["scenarios"].as_array().unwrap();
        assert_eq!(sc.len(), 11);
        for s in sc {
            assert!(run(s, false), "{}", s["name"]);
            assert_eq!(!run(s, true), s["baseFails"].as_bool().unwrap(), "12.1.15 control: {}", s["name"]);
        }
    }

    /// 4-9 (review): an earlier unmute that takes the lock after the mode change's mute does nothing; a rebuild started
    /// before the mute publishes muted.
    #[test]
    fn a_late_write_never_reopens_the_mic() {
        // the hold's unmute = write 1, the mode change's mute = write 2; write 2 runs first, then write 1
        let (unmute, mute) = (1u64, 2u64);
        assert!(write_is_current(mute, 2), "the newest write applies");
        assert!(!write_is_current(unmute, 2), "the late, older unmute skips");
        assert!(rebuild_muted(false, 1, 2, true), "a rebuild captured open before the mute publishes muted");
        assert!(!rebuild_muted(false, 2, 2, true), "without a newer write it keeps its own state");
        let cc = include_str!("call_ctl.rs");
        let am = &cc[cc.find("fn apply_mic(mut self").unwrap()..];
        let am = &am[..am.find("\n    }\n").unwrap()];
        assert!(am.contains("crate::mic_mode::write_is_current(gen, MIC_APPLY_GEN.load("), "{am}");
        let rp = &cc[cc.find("fn republish_live_mic(was_muted: bool)").unwrap()..];
        assert!(rp[..rp.find("\n}\n").unwrap()].contains("crate::mic_mode::rebuild_muted("));
    }

    #[test]
    fn the_vectors_are_the_webs() {
        let Ok(dir) = std::env::var("TINHAT_WEB_DIR") else { return };
        let web = std::fs::read_to_string(format!("{dir}/src/features/calls/testvectors/mic-mode.json")).unwrap();
        assert_eq!(web, V, "byte-identical with the web's copy");
    }

    /// The controller asks these rules: the setting (and its button) and an explicit Leave.
    #[test]
    fn the_controller_asks_the_rules() {
        let cc = include_str!("call_ctl.rs");
        let en = &cc[cc.find("pub fn enable_push_to_talk(").unwrap()..];
        let en = &en[..en.find("\n    }\n").unwrap()];
        assert!(en.contains("crate::mic_mode::ptt_toggled(on)") && en.contains("PTT_TAIL_GEN.fetch_add(1"), "{en}");
        let lv = &cc[cc.find("pub fn leave_call(").unwrap()..];
        let lv = &lv[..lv.find("watch_ledger().press_leave();").unwrap()];
        assert!(lv.contains("crate::mic_mode::leave_hold(*self.push_to_talk(), *self.mic_muted())"), "{lv}");
        // (review) a push-to-talk change drops BRB's restore claim (AppCore's AFK edge would reopen the mic on return)
        let core = include_str!("../qml/AppCore.qml");
        assert!(core.contains("function onPushToTalkChanged() { core._afkMutedMic = false }"));
    }
}
