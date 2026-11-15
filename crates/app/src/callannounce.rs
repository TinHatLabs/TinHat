//! What a screen reader hears about calls (accessibility round 2, M3). Pure rules, no I/O: the text for an incoming
//! call, for the call's own state changes (joining, in, reconnecting, failed, left) and for people joining / leaving
//! it, and whether it interrupts (urgent) or waits (polite). The web's src/features/calls/callannounce.ts, same rules,
//! same vectors (testvectors/callannounce.json = web callannounce.vectors.json, byte-identical). QML reaches them
//! through CallAnnounceController (callannounce_ctl.rs) and says them with Accessible.announce.

use serde::Deserialize;

/// People joining / leaving in the first ROSTER_ARM_MS after you got in are the people already there: not announced
/// (the join chime's window).
pub const ROSTER_ARM_MS: u32 = 2500;
/// Joins and leaves within this window are read as one sentence.
pub const ROSTER_BATCH_MS: u32 = 1200;

/// The call flags both apps keep (AppCore mInCall, mConnecting, … / the web's CallState).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct CallFlags {
    pub in_call: bool,
    pub connecting: bool,
    pub reconnecting: bool,
    pub call_failed: bool,
    pub fail_reason: String,
    pub removed_by: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallPhase {
    Idle,
    Connecting,
    In,
    Reconnecting,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    pub text: String,
    /// true = assertive (interrupts); false = polite.
    pub urgent: bool,
}

fn say(text: String, urgent: bool) -> Option<Announcement> {
    Some(Announcement { text, urgent })
}

/// One phase from the flags: failed > reconnecting > in > connecting > idle.
pub fn call_phase(f: &CallFlags) -> CallPhase {
    if f.call_failed {
        CallPhase::Failed
    } else if f.reconnecting {
        CallPhase::Reconnecting
    } else if f.in_call {
        CallPhase::In
    } else if f.connecting {
        CallPhase::Connecting
    } else {
        CallPhase::Idle
    }
}

fn in_room(room: &str) -> String {
    let r = room.trim();
    if r.is_empty() { String::new() } else { format!(" in {r}") }
}

fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// The failure banner's own words (CallIsland.qml / CallStage.tsx), with the reason when a join failed.
pub fn fail_text(f: &CallFlags) -> String {
    let by = f.removed_by.trim();
    if !by.is_empty() {
        return format!("{by} disconnected you from the call");
    }
    let r = f.fail_reason.trim();
    if !r.is_empty() {
        return format!("Couldn't join the call: {}", capitalised(r));
    }
    "Call ended — connection lost".to_string()
}

/// What to say when the call's state goes from `prev` to `next` (None = nothing to say).
pub fn phase_announcement(prev: &CallFlags, next: &CallFlags, room: &str) -> Option<Announcement> {
    let a = call_phase(prev);
    let b = call_phase(next);
    if b == CallPhase::Failed {
        if a == CallPhase::Failed && fail_text(prev) == fail_text(next) {
            return None;
        }
        return say(fail_text(next), true);
    }
    if a == b {
        return None;
    }
    match b {
        CallPhase::Connecting => say(format!("Joining the call{}…", in_room(room)), false),
        CallPhase::In if a == CallPhase::Reconnecting => say("Reconnected to the call".into(), false),
        CallPhase::In => say(format!("You're in the call{}", in_room(room)), false),
        CallPhase::Reconnecting => say("Reconnecting to the call…".into(), true),
        CallPhase::Idle => match a {
            CallPhase::Failed => None,
            CallPhase::Connecting => say("Stopped joining the call".into(), false),
            _ => say("You left the call".into(), false),
        },
        CallPhase::Failed => None,
    }
}

/// An incoming call card appeared.
pub fn incoming_announcement(room: &str, who: &str, in_other_call: bool) -> Announcement {
    let w = who.trim();
    let w = if w.is_empty() { "Someone started a call".to_string() } else { format!("{w} started a call") };
    let other = if in_other_call { ". You're in another call" } else { "" };
    Announcement { text: format!("Incoming call{}: {w}{other}", in_room(room)), urgent: true }
}

/// "Ana", "Ana and Ben", "Ana, Ben and Cy", "Ana, Ben and 2 others" (the join banner's way of counting).
pub fn name_list(names: &[String]) -> String {
    let n: Vec<&str> = names.iter().map(|x| x.trim()).filter(|x| !x.is_empty()).collect();
    match n.len() {
        0 => String::new(),
        1 => n[0].to_string(),
        2 => format!("{} and {}", n[0], n[1]),
        3 => format!("{}, {} and {}", n[0], n[1], n[2]),
        k => format!("{}, {} and {} others", n[0], n[1], k - 2),
    }
}

/// A15.2/A15.6 (2026-11-13): the incoming card's two lines (title, sub). A DM names the caller ("Ana is calling"); a
/// group names the room and who is in the call now (`in_call`, me left out), else who started it. = the web's incomingCard.
pub fn incoming_card(dm: bool, room: &str, who: &str, in_call: &[String], in_other_call: bool) -> (String, String) {
    let other = if in_other_call { " — you're in another call" } else { "" };
    let w = who.trim();
    if dm {
        let n = if w.is_empty() { room.trim() } else { w };
        let n = if n.is_empty() { "Someone" } else { n };
        return (format!("{n} is calling"), format!("Incoming call{other}"));
    }
    let list = name_list(in_call);
    let count = in_call.iter().filter(|x| !x.trim().is_empty()).count();
    let sub = if !list.is_empty() {
        format!("{list} {} in the call", if count == 1 { "is" } else { "are" })
    } else if !w.is_empty() {
        format!("{w} started a call")
    } else {
        "Someone started a call".to_string()
    };
    (format!("Incoming call · {}", room.trim()), format!("{sub}{other}"))
}

/// D09.5 (2026-11-13): after I START a call (`alone`: nobody else in it yet) in a room with members on other servers, they
/// will meet "Join a call hosted elsewhere?" — say so to the caller. "" = nothing to say. = the web's startNote.
pub const START_NOTE: &str = "People here from other servers will be asked to trust your server's call host before they can join.";
pub fn start_note(me: &str, members: &[String], alone: bool) -> &'static str {
    fn server(m: &str) -> Option<&str> {
        let m = m.strip_prefix('@')?;
        let (_, s) = m.split_once(':')?;
        (!s.is_empty()).then_some(s)
    }
    let Some(mine) = server(me) else { return "" };
    if alone && members.iter().filter_map(|m| server(m)).any(|s| s != mine) { START_NOTE } else { "" }
}

/// People joined / left the call I am in (one batch).
pub fn roster_announcement(joined: &[String], left: &[String]) -> Option<Announcement> {
    let mut parts = Vec::new();
    let j = name_list(joined);
    let l = name_list(left);
    if !j.is_empty() {
        parts.push(format!("{j} joined the call"));
    }
    if !l.is_empty() {
        parts.push(format!("{l} left the call"));
    }
    if parts.is_empty() {
        return None;
    }
    say(parts.join(". "), false)
}

/// An announcement as the JSON QML reads: {"text": …, "urgent": …}, or "" for nothing.
pub fn to_json(a: &Option<Announcement>) -> String {
    match a {
        Some(a) => serde_json::json!({ "text": a.text, "urgent": a.urgent }).to_string(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn vectors() -> Value {
        serde_json::from_str(include_str!("testvectors/callannounce.json")).expect("vectors")
    }
    fn out(v: &Value) -> Option<Announcement> {
        if v.is_null() {
            return None;
        }
        Some(Announcement { text: v["text"].as_str().unwrap().to_string(), urgent: v["urgent"].as_bool().unwrap() })
    }
    fn strs(v: &Value) -> Vec<String> {
        v.as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn constants_match_the_vectors() {
        let v = vectors();
        assert_eq!(v["constants"]["rosterArmMs"].as_u64().unwrap(), ROSTER_ARM_MS as u64);
        assert_eq!(v["constants"]["rosterBatchMs"].as_u64().unwrap(), ROSTER_BATCH_MS as u64);
    }

    #[test]
    fn phase_every_vector() {
        for c in vectors()["phase"].as_array().unwrap() {
            let p: CallFlags = serde_json::from_value(c["prev"].clone()).unwrap();
            let n: CallFlags = serde_json::from_value(c["next"].clone()).unwrap();
            assert_eq!(phase_announcement(&p, &n, c["room"].as_str().unwrap()), out(&c["out"]), "{}", c["name"]);
        }
    }

    #[test]
    fn card_every_vector() {
        let cards = vectors()["card"].as_array().unwrap().clone();
        assert_eq!(cards.len(), 8);
        for c in cards {
            let got = incoming_card(c["dm"].as_bool().unwrap(), c["room"].as_str().unwrap(), c["who"].as_str().unwrap(), &strs(&c["inCall"]), c["inOtherCall"].as_bool().unwrap());
            assert_eq!((got.0.as_str(), got.1.as_str()), (c["out"]["title"].as_str().unwrap(), c["out"]["sub"].as_str().unwrap()), "{}", c["name"]);
        }
    }

    #[test]
    fn start_note_every_vector() {
        let cases = vectors()["startNote"].as_array().unwrap().clone();
        assert_eq!(cases.len(), 5);
        for c in cases {
            assert_eq!(start_note(c["me"].as_str().unwrap(), &strs(&c["members"]), c["alone"].as_bool().unwrap()), c["out"].as_str().unwrap(), "{}", c["name"]);
        }
        let cc = include_str!("call_ctl.rs");
        assert!(cc.contains("crate::callannounce::start_note(&me, &members, alone)"), "wired after the join");
    }

    /// D05.5: the call bar's toggles say whether they are on, and each ▴ is a named, focusable button (= the web's
    /// aria-pressed + menuName).
    #[test]
    fn call_buttons_say_their_state() {
        let cb = include_str!("../qml/CallButton.qml");
        assert!(cb.contains("Accessible.checkable: cb.checkable") && cb.contains("Accessible.checked: cb.checked"));
        assert!(cb.contains("activeFocusOnTab: cb.hasMenu") && cb.contains("Accessible.name: cb.menuName") && cb.contains("Keys.onSpacePressed: cb.menuClicked()"));
        let ci = include_str!("../qml/CallIsland.qml");
        for k in ["checked: app.mMicMuted || app.mServerMuted", "checked: app.mDeafened", "checked: app.mCameraOn", "checked: app.mCallSharing", "checked: app.mMusicPlaying"] {
            assert!(ci.contains(k), "{k}");
        }
        for n in ["Microphone settings", "Speaker settings", "Camera settings", "Screen share settings", "Shared audio settings"] {
            assert!(ci.contains(&format!("menuName: \"{n}\"")), "{n}");
        }
    }

    /// A22.5: the push-to-talk setting has "Change key…", which opens Shortcuts filtered to it (= the web).
    #[test]
    fn ptt_change_key() {
        let sw = include_str!("../qml/SettingsWindow.qml");
        assert!(sw.contains("onClicked: { win.shortcutsPreset = \"Push to talk\"; win.currentKey = \"shortcuts\" }") && sw.contains("preset: win.shortcutsPreset"));
        let sc = include_str!("../qml/ShortcutsPane.qml");
        assert!(sc.contains("shortcutsSearch.text = pane.preset"));
    }

    /// A12.2: the mute / deafen tooltips name the key the action has now (= the web's helpKey).
    #[test]
    fn call_tips_follow_rebinding() {
        let ci = include_str!("../qml/CallIsland.qml");
        assert!(ci.contains("app.helpKey(\"call.mic.toggle\", \"Ctrl+Shift+M\")") && ci.contains("app.helpKey(\"call.deafen.toggle\", \"Ctrl+Shift+D\")"));
        assert!(!ci.contains("\" — Ctrl+Shift+M\"") && !ci.contains("\" — Ctrl+Shift+D\""));
        let main = include_str!("../qml/Main.qml");
        assert!(main.contains("function helpKey(id, def) { return actionsHost.helpKey(id, def) }"));
    }

    /// A18.3: a clip jump starts 5 s before the press (= the web's clipjump.ts).
    #[test]
    fn clip_jumps_lead_in() {
        let m = include_str!("../qml/MemoriesWindow.qml");
        assert!(m.contains("readonly property int clipLeadInS: 5") && m.contains("function clipSeekOf(off) { return Math.max(0, off - mem.clipLeadInS) }"));
        assert_eq!(m.matches("mem.commitSeek(mem.clipSeekOf(parent.modelData.off))").count(), 2);
    }

    /// A15.3: past the ring the card settles (top right, narrower, one line) (= the web's Incoming.tsx settled).
    #[test]
    fn the_card_settles() {
        let q = include_str!("../qml/IncomingCallToast.qml");
        assert!(q.contains("readonly property bool settled:") && q.contains("n - since < 25000") && q.contains("visible: !toast.settled"));
        let m = include_str!("../qml/Main.qml");
        assert!(m.contains("x: incomingToast.settled ? parent.width - width - Kirigami.Units.gridUnit : (parent.width - width) / 2"));
    }

    /// A15.1: the header's join banner shows in another room's call, with Switch (= the web's JoinBanner).
    #[test]
    fn join_banner_offers_switch() {
        let h = include_str!("../qml/HeaderIsland.qml");
        assert!(h.contains("(!island.app.mInCall || island.inOtherCall)") && h.contains("island.inOtherCall ? \"Switch\"") && h.contains("app.callctl.switchCall(app.room.roomId)"));
    }

    /// A15.2/A15.6: the desktop card asks the rule (= the web's Incoming.tsx CardLines).
    #[test]
    fn the_card_asks_the_rule() {
        let card = include_str!("../qml/IncomingCallToast.qml");
        assert!(card.contains("app.incomingCardJson(") && card.contains("text: parent.lines.title") && card.contains("text: parent.lines.sub"));
        let core = include_str!("../qml/AppCore.qml");
        assert!(core.contains("return announceCtl.card("));
    }

    #[test]
    fn incoming_every_vector() {
        for c in vectors()["incoming"].as_array().unwrap() {
            let a = incoming_announcement(c["room"].as_str().unwrap(), c["who"].as_str().unwrap(), c["inOtherCall"].as_bool().unwrap());
            assert_eq!(Some(a), out(&c["out"]), "{}", c["name"]);
        }
    }

    #[test]
    fn names_and_roster_every_vector() {
        let v = vectors();
        for c in v["names"].as_array().unwrap() {
            assert_eq!(name_list(&strs(&c["in"])), c["out"].as_str().unwrap());
        }
        for c in v["roster"].as_array().unwrap() {
            assert_eq!(roster_announcement(&strs(&c["joined"]), &strs(&c["left"])), out(&c["out"]), "{}", c["name"]);
        }
    }

    #[test]
    fn json_for_qml() {
        assert_eq!(to_json(&None), "");
        let j: Value = serde_json::from_str(&to_json(&Some(Announcement { text: "a \"b\"".into(), urgent: true }))).unwrap();
        assert_eq!(j["text"], "a \"b\"");
        assert_eq!(j["urgent"], true);
    }

    const WEB_COPY: &str = "src/features/calls/callannounce.vectors.json";

    /// A clean checkout without the web repo compares nothing here; the parity job runs the required test below.
    #[test]
    fn the_webs_copy_of_the_vectors_is_byte_identical_when_present() {
        let mine = include_bytes!("testvectors/callannounce.json");
        let dir = std::env::var("TINHAT_WEB_DIR").unwrap_or_else(|_| format!("{}/tinhat-web", std::env::var("HOME").unwrap_or_default()));
        if let Ok(b) = std::fs::read(format!("{dir}/{WEB_COPY}")) {
            assert_eq!(&b[..], &mine[..], "{dir}/{WEB_COPY}");
        }
    }

    /// The parity job: `TINHAT_WEB_DIR=… cargo test -p tinhat-app callannounce -- --include-ignored` — the web's copy
    /// MUST be there and equal (fails without TINHAT_WEB_DIR).
    #[test]
    #[ignore = "parity job: needs TINHAT_WEB_DIR (the web repo)"]
    fn parity_the_webs_copy_of_the_vectors_is_required_and_identical() {
        let mine = include_bytes!("testvectors/callannounce.json");
        let dir = std::env::var("TINHAT_WEB_DIR").expect("TINHAT_WEB_DIR must name the web repo for the parity job");
        let p = format!("{dir}/{WEB_COPY}");
        let b = std::fs::read(&p).unwrap_or_else(|e| panic!("{p}: {e}"));
        assert_eq!(&b[..], &mine[..], "{p}");
    }
}
