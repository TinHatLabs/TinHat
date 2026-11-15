//! CallAnnounceController — the call announcement rules (callannounce.rs, = the web's callannounce.ts) for QML
//! (accessibility round 2, M3). QML passes the call flags as JSON ({inCall, connecting, reconnecting, callFailed,
//! failReason, removedBy}) and gets back "" (nothing to say) or {"text": …, "urgent": …}, which AppCore says with
//! Accessible.announce (urgent = assertive).

use crate::callannounce::{self, CallFlags, CallPhase};
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qobject {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name] // snake_case Rust -> camelCase QML
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        type CallAnnounceController = super::CallAnnounceControllerRust;

        /// QML: phase(prevJson, nextJson, room) — the call's own state change ("" = nothing to say).
        #[qinvokable]
        fn phase(self: &CallAnnounceController, prev: &QString, next: &QString, room: &QString) -> QString;
        /// QML: incoming(room, who, inOtherCall) — an incoming call card appeared (always urgent).
        #[qinvokable]
        fn incoming(self: &CallAnnounceController, room: &QString, who: &QString, in_other_call: bool) -> QString;
        /// QML: roster(joinedJson, leftJson) — one batch of people joining / leaving ("" = nothing).
        #[qinvokable]
        fn roster(self: &CallAnnounceController, joined: &QString, left: &QString) -> QString;
        /// A15.2/A15.6: the incoming card's lines as {"title", "sub"} (callannounce::incoming_card)
        #[qinvokable]
        fn card(self: &CallAnnounceController, dm: bool, room: &QString, who: &QString, in_call: &QString, in_other_call: bool) -> QString;
        /// QML: phaseOf(flagsJson) — "idle" | "connecting" | "in" | "reconnecting" | "failed".
        #[qinvokable]
        fn phase_of(self: &CallAnnounceController, flags: &QString) -> QString;
        /// QML: num("rosterArmMs" | "rosterBatchMs") — the shared windows (0 for an unknown name).
        #[qinvokable]
        fn num(self: &CallAnnounceController, name: &QString) -> i32;
    }
}

#[derive(Default)]
pub struct CallAnnounceControllerRust;

fn flags(s: &QString) -> CallFlags {
    serde_json::from_str(&s.to_string()).unwrap_or_default()
}
fn names(s: &QString) -> Vec<String> {
    serde_json::from_str(&s.to_string()).unwrap_or_default()
}

impl qobject::CallAnnounceController {
    fn phase(&self, prev: &QString, next: &QString, room: &QString) -> QString {
        QString::from(callannounce::to_json(&callannounce::phase_announcement(&flags(prev), &flags(next), &room.to_string())).as_str())
    }
    fn incoming(&self, room: &QString, who: &QString, in_other_call: bool) -> QString {
        let a = callannounce::incoming_announcement(&room.to_string(), &who.to_string(), in_other_call);
        QString::from(callannounce::to_json(&Some(a)).as_str())
    }
    fn card(&self, dm: bool, room: &QString, who: &QString, in_call: &QString, in_other_call: bool) -> QString {
        let (title, sub) = callannounce::incoming_card(dm, &room.to_string(), &who.to_string(), &names(in_call), in_other_call);
        QString::from(serde_json::json!({ "title": title, "sub": sub }).to_string().as_str())
    }
    fn roster(&self, joined: &QString, left: &QString) -> QString {
        QString::from(callannounce::to_json(&callannounce::roster_announcement(&names(joined), &names(left))).as_str())
    }
    fn phase_of(&self, f: &QString) -> QString {
        let p = match callannounce::call_phase(&flags(f)) {
            CallPhase::Idle => "idle",
            CallPhase::Connecting => "connecting",
            CallPhase::In => "in",
            CallPhase::Reconnecting => "reconnecting",
            CallPhase::Failed => "failed",
        };
        QString::from(p)
    }
    fn num(&self, name: &QString) -> i32 {
        match name.to_string().as_str() {
            "rosterArmMs" => callannounce::ROSTER_ARM_MS as i32,
            "rosterBatchMs" => callannounce::ROSTER_BATCH_MS as i32,
            _ => 0,
        }
    }
}
