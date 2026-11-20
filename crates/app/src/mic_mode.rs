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

/// R2 (review round): the mic writes' generation and that write's value in one u64, so the call controller keeps them
/// in ONE atomic (a reader never sees a new generation with the previous value).
pub(crate) const fn pack_mic(gen: u64, muted: bool) -> u64 {
    (gen << 1) | muted as u64
}
/// -> (generation, muted)
pub(crate) const fn unpack_mic(v: u64) -> (u64, bool) {
    (v >> 1, v & 1 == 1)
}

/// R2 (assembly-1216 re-review): what ANY task that captured a mic state publishes once it holds the call lock (a mid-call
/// AGC / noise / echo / device rebuild, the moderator's-unmute restore, Reconnect mic, the join's mute): 4-9's rule
/// ([`rebuild_muted`]), and muted while a moderator's mute holds. Every mic choice is a generation, a server-blocked
/// unmute too (call_ctl.rs apply_mic), so a task that started before the person's newest choice publishes that choice.
pub(crate) fn late_write_muted(captured: bool, gen_at_capture: u64, gen_now: u64, newest: bool, server_muted: bool) -> bool {
    server_muted || rebuild_muted(captured, gen_at_capture, gen_now, newest)
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
        assert!(am.contains("crate::mic_mode::write_is_current(gen, mic_gen())"), "{am}");
        let rp = &cc[cc.find("fn republish_live_mic(was_muted: bool)").unwrap()..];
        assert!(rp[..rp.find("\n}\n").unwrap()].contains("mic_muted_at_lock(was_muted, gen0)"));
    }

    /// R2: the task-order model of the controller's mic writes (call_ctl.rs apply_mic, the late tasks), run on the two
    /// interleavings the re-review found, under today's rules and the 12.1.16 ones (the control: those end with the UI
    /// live and the publication muted).
    #[derive(Default)]
    struct Ctl {
        gen: u64,
        newest: bool,
        server_muted: bool, // the streamer's i_am_server_muted / the UI's serverMuted
        ui_muted: bool,     // micMuted as the person sees it
        published_muted: bool,
        base: bool, // 12.1.16's rules
    }
    enum Task {
        Write(u64, bool),     // apply_mic's own write: (its generation, muted)
        Late(bool, u64, bool), // a captured state: (muted, generation at capture, base: published as captured)
    }
    impl Ctl {
        /// apply_mic: the person's choice
        fn choose(&mut self, muted: bool) -> Option<Task> {
            self.ui_muted = muted;
            if self.base && !muted && self.server_muted {
                return None; // 12.1.16: no generation for a blocked unmute
            }
            self.gen += 1;
            self.newest = muted;
            if !muted && self.server_muted {
                return None;
            }
            Some(Task::Write(self.gen, muted))
        }
        /// a rebuild (`base_checked` = it had 4-9's check in 12.1.16: the AGC rebuild did, the moderator restore didn't)
        fn capture(&self, muted: bool, base_checked: bool) -> Task {
            Task::Late(muted, self.gen, self.base && !base_checked)
        }
        fn run(&mut self, t: Task) {
            match t {
                Task::Write(g, m) => {
                    if write_is_current(g, self.gen) {
                        self.published_muted = m;
                    }
                }
                Task::Late(m, g0, raw) => {
                    self.published_muted = if raw {
                        m
                    } else if self.base {
                        rebuild_muted(m, g0, self.gen, self.newest)
                    } else {
                        late_write_muted(m, g0, self.gen, self.newest, self.server_muted)
                    };
                }
            }
        }
        /// the R2 symptom: the UI says live, the publication is muted (or the reverse: a hot mic)
        fn consistent(&self) -> bool {
            self.server_muted || self.ui_muted == self.published_muted
        }
    }
    /// interleaving 1: the moderator unmutes someone whose push-to-talk key is up; the restore captured "muted"; the key
    /// goes down and its write runs first; then the restore lands
    fn restore_vs_hold(base: bool) -> bool {
        let mut c = Ctl { server_muted: true, ui_muted: true, published_muted: true, newest: true, base, ..Default::default() };
        c.server_muted = false;
        let restore = c.capture(true, false);
        let hold = c.choose(false).expect("a write");
        c.run(hold);
        c.run(restore);
        c.consistent()
    }
    /// interleaving 2: server-muted with the key up, an AGC change captures "muted"; the key goes down (blocked: the
    /// moderator's mute holds); the moderator unmutes and the restore publishes live; then the AGC rebuild lands
    fn agc_vs_blocked_unmute(base: bool) -> bool {
        let mut c = Ctl { server_muted: true, ui_muted: true, published_muted: true, newest: true, base, ..Default::default() };
        let agc = c.capture(true, true);
        assert!(c.choose(false).is_none(), "the unmute is blocked: nothing reaches the SFU");
        c.server_muted = false;
        let restore = c.capture(c.ui_muted, false);
        c.run(restore);
        c.run(agc);
        c.consistent()
    }
    #[test]
    fn r2_the_packed_generation_round_trips() {
        for (g, m) in [(0u64, true), (0, false), (1, false), (7, true), (u64::MAX >> 1, true), (u64::MAX >> 1, false)] {
            assert_eq!(unpack_mic(pack_mic(g, m)), (g, m));
        }
        assert_eq!(unpack_mic(pack_mic(41, true)).0 + 1, 42);
    }

    #[test]
    fn r2_a_late_mic_task_publishes_the_newest_choice() {
        assert!(restore_vs_hold(false), "today: the restore publishes the hold's live mic");
        assert!(agc_vs_blocked_unmute(false), "today: the AGC rebuild publishes the blocked unmute's live mic");
        // the control: 12.1.16's rules end silent with the UI live, in both
        assert!(!restore_vs_hold(true), "12.1.16 control: restore vs hold");
        assert!(!agc_vs_blocked_unmute(true), "12.1.16 control: AGC vs blocked unmute");
        // and never hot: while a moderator's mute holds, a late task publishes muted whatever it captured
        assert!(late_write_muted(false, 1, 1, false, true));
        assert!(late_write_muted(false, 1, 2, false, true));
        assert!(!late_write_muted(false, 1, 1, true, false), "no newer write: its own state");
        assert!(!late_write_muted(true, 1, 2, false, false), "a newer write: that write's state");
    }

    /// R2: every task in call_ctl.rs that publishes a mic state it captured earlier asks mic_muted_at_lock under the call
    /// lock (the re-review named the moderator's restore and the AGC / denoise changes; Reconnect mic, the echo / speaker
    /// / device changes and the join's mute captured the same way). apply_mic's own write has its generation check, and
    /// a blocked unmute is a new generation.
    #[test]
    fn r2_every_late_mic_write_asks_the_rule() {
        let all = include_str!("call_ctl.rs");
        let ctl = &all[..all.find("\n#[cfg(test)]").expect("the tests")];
        let mut seen = 0;
        for pat in ["call.set_mic_muted(", "call.republish_mic(", "call.publish_mic_muted("] {
            for (i, _) in ctl.match_indices(pat) {
                // the task that holds it: from its spawn to the write
                let spawn = ctl[..i].rfind("crate::bridge::runtime().spawn(async move {").expect("a task");
                let task = &ctl[spawn..i];
                let line = &ctl[i..i + ctl[i..].find('\n').unwrap()];
                if task.contains("crate::mic_mode::write_is_current(gen, mic_gen())") {
                    continue; // apply_mic's own write
                }
                if line.contains("publish_mic_muted(start_muted || barrier)") {
                    continue; // the join's first publish; its mute is set right after by the join's checked write
                }
                assert!(task.contains("mic_muted_at_lock("), "a late mic write without R2's rule: {line}");
                let lock = task.rfind("cell.lock().await").expect("under the call lock");
                assert!(task[lock..].contains("mic_muted_at_lock("), "decided after the lock is taken: {line}");
                seen += 1;
            }
        }
        assert!(seen >= 9, "the late writes found: {seen}");
        let am = &ctl[ctl.find("fn apply_mic(mut self").unwrap()..];
        let am = &am[..am.find("\n    }\n").unwrap()];
        let gen = am.find("MIC_APPLY.store(crate::mic_mode::pack_mic(gen, muted)").expect("a generation per choice");
        assert!(gen < am.find("if !muted && *self.server_muted() {").unwrap(), "a blocked unmute is a generation too");
        // the generation and its value are one atomic (review round), and apply_mic's own write keeps a moderator's mute
        assert!(!ctl.contains("MIC_APPLY_GEN") && !ctl.contains("MIC_APPLIED_MUTED"), "no second atomic");
        assert!(am.contains("call.set_mic_muted(muted || tinhat_streamer::call::i_am_server_muted());"));
        let f = &ctl[ctl.find("fn mic_muted_at_lock(").unwrap()..];
        assert!(f[..f.find("\n}\n").unwrap()].contains("tinhat_streamer::call::i_am_server_muted()"));
        // the gate's lab steps: refused unless lab hooks are on (a release build never has them), the hold compiled out
        let main = include_str!("../qml/Main.qml");
        let refuse = main.find("else if ((s.do === \"mic-late-hold\" || s.do === \"mic-agc\") && !login.labDriveHooks()) { r = \"refused\" }").expect("refused outside a lab build");
        assert!(refuse < main.find("else if (s.do === \"mic-late-hold\") {").unwrap() && refuse < main.find("else if (s.do === \"mic-agc\") {").unwrap());
        let hold = &all[all.find("pub fn lab_mic_late_hold(&self, ms: i32) -> bool {").unwrap()..];
        let hold = &hold[..hold.find("\n    }\n").unwrap()];
        assert!(hold.contains("#[cfg(tinhat_env_selftests)]") && hold.contains("#[cfg(not(tinhat_env_selftests))]") && hold.contains("TINHAT_LABDRIVE_MICHOLD"));
        let rbc = include_str!("../../../scripts/release-binary-check.sh");
        assert!(rbc.contains(" TINHAT_LABDRIVE_MICHOLD tinhat-lab-mic-late-hold "), "release-binary-check refuses both names");
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
