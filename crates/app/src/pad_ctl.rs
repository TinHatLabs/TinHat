//! pad_ctl.rs — PadController: Pad's bridge to QML (docs/PAD.md; DESIGN §6 "Desktop").
//!
//! One instance (AppCore.qml `padCtl`). It owns nothing but the UI mirrors; the work is the process-wide
//! [`PadNode`](crate::pad_net::PadNode) of the signed-in session (created once a session exists and the UI attached,
//! replaced when the session changes):
//!   * RECEIVER — the sync loop's Pad branch (bridge.rs) lands in [`on_to_device`]; a command the rules accept is
//!     run by QML (`exec(token, cmds)` → PadHost.qml runs the SAME functions the buttons run, then
//!     `executed(token)`); QML publishes this device's call snapshot (`publishState`); `chipsJson` = the devices with
//!     a live Pad session here (the "📱 Pad · {device} ×" chip; × = `refuseDevice`); `firstPad` = the one-time toast.
//!   * CONTROLLER — `refreshTargets()` → `targetsJson` (my other devices in each call: discovery + why not); the
//!     Pad window opens a session (`openPad`), sends commands (`send` → seq), pings while visible (`setVisible`),
//!     closes it (`closePad`); `remoteJson` = that session's newest state + reach + pending commands; `acked` /
//!     `failed` / `ended` drive the optimistic UI.
//!   * Settings → Calls → Pad: `padOn` / `applyPadOn` (default on; persisted as the `pad-allow` pref, the web's key).
//!     A change re-publishes the call membership at once (the `pad1` cap, rtc.rs); off ends every session (`off`) and
//!     refuses every command.
//! Every QML-facing function here is `#[qinvokable]` (the 9.x boundary trap: a `#[cxx_name]` alone is invisible
//! to QML); signals are `#[qsignal]`. cxx-qt property notifications don't reliably drive QML bindings in this app,
//! so every change also bumps `rev` and emits `changed()` (AppCore mirrors on both).
use core::pin::Pin;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use cxx_qt::{CxxQtThread, Threading};
use cxx_qt_lib::QString;
use serde_json::{json, Value};

use crate::pad_net::{PadEvent, PadNode, Sink};

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
        #[qproperty(bool, pad_on)] //          QML: padOn — Settings → Calls → Pad (this device may be controlled)
        #[qproperty(QString, chips_json)] //   QML: chipsJson — [{device, name}] my devices with a live Pad session here
        #[qproperty(QString, targets_json)] // QML: targetsJson — {rooms: {roomId: [{device,name,ready,why,createdTs}]}, names, verified}
        #[qproperty(QString, remote_json)] //  QML: remoteJson — the session this device drives (null = none)
        #[qproperty(QString, sounds_json)] //  QML: soundsJson — {room, roomSounds: [{name,url,emoji}], people: [{mxid,name,avatar}]}
        #[qproperty(i32, rev)] //              QML: rev — bumps on every change of the above
        type PadController = super::PadControllerRust;

        /// Receiver: run these commands (`[{do, …}]`) exactly as the buttons do, then call executed(token).
        #[qsignal]
        fn exec(self: Pin<&mut PadController>, token: i32, cmds: QString);
        /// Controller: the receiver answered command `seq`.
        #[qsignal]
        fn acked(self: Pin<&mut PadController>, seq: i32, ok: bool, why: QString, op: QString);
        /// Controller: command `seq` got no answer after its resend (roll back; "Not reaching {device}").
        #[qsignal]
        fn failed(self: Pin<&mut PadController>, seq: i32, op: QString);
        /// Controller: the session ended there ("replaced" | "closed" | "off" | "unreachable" | a refused open's code).
        #[qsignal]
        fn ended(self: Pin<&mut PadController>, why: QString);
        /// Receiver: a device of mine started a Pad here for the first time ever (the one-time toast).
        #[qsignal]
        fn first_pad(self: Pin<&mut PadController>, device: QString, name: QString);
        /// Something above changed (rev bumped).
        #[qsignal]
        fn changed(self: Pin<&mut PadController>);

        /// QML: attach() — once, from AppCore's Component.onCompleted: events reach this object from now on.
        #[qinvokable]
        fn attach(self: Pin<&mut PadController>);
        /// QML: applyPadOn(on) — Settings → Calls → Pad (persisted; off ends every session here with `off`).
        #[qinvokable]
        fn apply_pad_on(self: Pin<&mut PadController>, on: bool);
        /// QML: publishState(json) — this device's call snapshot (PadHost.snapshot()).
        #[qinvokable]
        fn publish_state(self: Pin<&mut PadController>, json: &QString);
        /// QML: claim(token) — PadHost is about to run `exec(token, …)`: true = run it now (the session is live, Pad on,
        /// the device not refused, the call it was accepted for is the call this device is in right now, ≤ 1.5 s old);
        /// false = run nothing (the refusal goes back from here).
        #[qinvokable]
        fn claim(self: Pin<&mut PadController>, token: i32) -> bool;
        /// QML: executed(token) — PadHost ran the commands of `exec(token, …)`.
        #[qinvokable]
        fn executed(self: Pin<&mut PadController>, token: i32);
        /// QML: refuseDevice(device) — × on the chip: end that device's Pad here for the rest of this call.
        #[qinvokable]
        fn refuse_device(self: Pin<&mut PadController>, device: &QString);
        /// QML: refreshTargets() — re-read my other devices in the calls (targetsJson).
        #[qinvokable]
        fn refresh_targets(self: Pin<&mut PadController>);
        /// QML: openPad(roomId, deviceId) — start driving that device's call.
        #[qinvokable]
        fn open_pad(self: Pin<&mut PadController>, room: &QString, device: &QString);
        /// QML: closePad() — the Pad window closed.
        #[qinvokable]
        fn close_pad(self: Pin<&mut PadController>);
        /// QML: setVisible(on) — the Pad window shown / hidden (pings; vis:false stops the pushes).
        #[qinvokable]
        fn set_visible(self: Pin<&mut PadController>, vis: bool);
        /// QML: send(json) — one command `{op, …}`; returns its seq (0 = not sent now: paced, or no open session).
        #[qinvokable]
        fn send(self: Pin<&mut PadController>, cmd: &QString) -> i32;
        /// QML: loadSounds(roomId) — the call room's sounds + who is in its call, for the Pad window (soundsJson).
        #[qinvokable]
        fn load_sounds(self: Pin<&mut PadController>, room: &QString);
        /// QML: whyText(code, device, op) — the one text per refusal code (padrules::why_text; "" = no text).
        #[qinvokable]
        fn why_text(self: &PadController, code: &QString, device: &QString, op: &QString) -> QString;
        /// QML: deviceName(deviceId) — my device's name from Settings → Devices & security (the id when unknown).
        #[qinvokable]
        fn device_name(self: &PadController, device: &QString) -> QString;
    }

    impl cxx_qt::Threading for PadController {}
}

pub struct PadControllerRust {
    pad_on: bool,
    chips_json: QString,
    targets_json: QString,
    remote_json: QString,
    sounds_json: QString,
    rev: i32,
}

impl Default for PadControllerRust {
    fn default() -> Self {
        PadControllerRust {
            pad_on: pad_enabled(),
            chips_json: QString::from("[]"),
            targets_json: QString::from("{\"rooms\":{},\"names\":{}}"),
            remote_json: QString::from("null"),
            sounds_json: QString::from("{}"),
            rev: 0,
        }
    }
}

/// Settings → Calls → Pad (`pad-allow`, default on). rtc.rs reads it for the `pad1` cap of the membership.
pub fn pad_enabled() -> bool {
    crate::settings::overlay_pref_read("pad-allow").map(|s| s != "0").unwrap_or(true)
}

struct Global {
    qt: Option<Arc<Mutex<CxxQtThread<qobject::PadController>>>>,
    node: Option<(u64, String, Arc<PadNode>)>,
}
fn global() -> &'static Mutex<Global> {
    static G: OnceLock<Mutex<Global>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(Global { qt: None, node: None }))
}
fn names() -> &'static Mutex<HashMap<String, String>> {
    static N: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    N.get_or_init(|| Mutex::new(HashMap::new()))
}
fn name_of(device: &str) -> String {
    names().lock().unwrap().get(device).cloned().unwrap_or_else(|| device.to_string())
}

/// The session's node (created on first use once a client exists and the UI attached). A new session (sign-out /
/// in, another account) gets a new node: nothing of one session acts for the next.
pub fn node() -> Option<Arc<PadNode>> {
    let Some(client) = crate::session::client() else {
        global().lock().unwrap().node = None; // signed out: the old session's node (and its client) goes
        return None;
    };
    let gen = crate::session::generation();
    let who = format!("{}|{}", client.user_id().map(|u| u.as_str()).unwrap_or(""), client.device_id().map(|d| d.as_str()).unwrap_or(""));
    let mut g = global().lock().unwrap();
    if let Some((ng, nw, n)) = g.node.as_ref() {
        if *ng == gen && *nw == who {
            return Some(n.clone());
        }
    }
    let qt = g.qt.clone()?;
    // the node's id travels with every event: one queued by a node that is no longer the session's is dropped
    let id = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let id2 = id.clone();
    let sink: Sink = Arc::new(move |ev: PadEvent| {
        if matches!(ev, PadEvent::Log(_)) {
            return; // already on stderr; never queued to the UI (an unauthenticated sender can't flood the Qt queue)
        }
        let from = id2.load(std::sync::atomic::Ordering::SeqCst);
        if let Ok(q) = qt.lock() {
            let _ = q.queue(move |o| {
                if current_node_id() == Some(from) {
                    on_event(o, ev)
                }
            });
        }
    });
    let n = PadNode::new(client, pad_enabled(), sink);
    id.store(n.id, std::sync::atomic::Ordering::SeqCst);
    names().lock().unwrap().clear(); // device names are per account
    let replaced = g.node.is_some();
    g.node = Some((gen, who, n.clone()));
    if replaced {
        // another session: the old one's mirrors go (chip, targets, the remote, the sounds)
        if let Some(Ok(q)) = g.qt.as_ref().map(|q| q.lock()) {
            let _ = q.queue(|mut o| {
                o.as_mut().set_chips_json(QString::from("[]"));
                o.as_mut().set_targets_json(QString::from("{\"rooms\":{},\"names\":{}}"));
                o.as_mut().set_remote_json(QString::from("null"));
                o.as_mut().set_sounds_json(QString::from("{}"));
                bump(o.as_mut());
            });
        }
    }
    Some(n)
}

/// The id of the session's current node — only while that node still belongs to the CURRENT session (sign-out / in
/// bumps the generation before any new node exists: an old node's queued events must not pass meanwhile).
fn current_node_id() -> Option<u64> {
    let g = global().lock().unwrap();
    g.node.as_ref().filter(|(gen, _, _)| *gen == crate::session::generation()).map(|(_, _, n)| n.id)
}

/// The sync loop's Pad branch (bridge.rs): every `dev.tinhat.pad.cmd` / `.state` to-device event, decrypted or not.
pub async fn on_to_device(pe: &matrix_sdk::deserialized_responses::ProcessedToDeviceEvent) {
    match node() {
        Some(n) => n.on_to_device(pe).await,
        None => eprintln!("[pad] a Pad message arrived before the window was ready — dropped"),
    }
}

/// The call generation the state carries (`gen`): this process's random tag + the call's join generation, so a
/// command meant for one call never lands in the next (a rejoin of the same room is a new generation).
fn gen_for(room: &str) -> String {
    static SALT: OnceLock<String> = OnceLock::new();
    let salt = SALT.get_or_init(|| crate::pad_net::mint_sid()[..8].to_string());
    match crate::call_ctl::key_intake_room() {
        Some((r, g)) if r == room => format!("{salt}{g:x}"),
        _ => format!("{salt}0"),
    }
}

/// The first-time toast: devices that ever started a Pad here (the `pad-seen` pref, at most 32).
fn first_time(device: &str) -> bool {
    let raw = crate::settings::overlay_pref_read("pad-seen").unwrap_or_default();
    let mut seen: Vec<String> = raw.split(',').filter(|s| !s.is_empty()).map(str::to_string).collect();
    if seen.iter().any(|s| s == device) {
        return false;
    }
    seen.push(device.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').take(64).collect());
    while seen.len() > 32 {
        seen.remove(0);
    }
    crate::settings::overlay_pref_write("pad-seen", &seen.join(","));
    true
}

fn bump(mut o: Pin<&mut qobject::PadController>) {
    let r = o.as_ref().rev().wrapping_add(1);
    o.as_mut().set_rev(r);
    o.as_mut().changed();
}

fn remote_json(n: &PadNode) -> String {
    let mut v = n.remote_view();
    if let Some(m) = v.as_object_mut() {
        let d = m.get("device").and_then(|d| d.as_str()).unwrap_or("").to_string();
        m.insert("name".into(), json!(name_of(&d)));
    }
    v.to_string()
}

fn on_event(mut o: Pin<&mut qobject::PadController>, ev: PadEvent) {
    match ev {
        PadEvent::Exec { token, cmds } => o.as_mut().exec(token as i32, QString::from(&cmds.to_string())),
        PadEvent::Chips(devs) => {
            let list: Vec<Value> = devs.iter().map(|d| json!({"device": d, "name": name_of(d)})).collect();
            o.as_mut().set_chips_json(QString::from(&Value::Array(list).to_string()));
            bump(o.as_mut());
            for d in &devs {
                if first_time(d) {
                    o.as_mut().first_pad(QString::from(d.as_str()), QString::from(&name_of(d)));
                }
            }
            if devs.iter().any(|d| !names().lock().unwrap().contains_key(d)) {
                o.as_mut().refresh_targets(); // a name we don't know yet
            }
        }
        PadEvent::Remote => {
            if let Some(n) = node() {
                o.as_mut().set_remote_json(QString::from(&remote_json(&n)));
                bump(o.as_mut());
            }
        }
        PadEvent::Ack { seq, ok, why, op } => o.as_mut().acked(seq as i32, ok, QString::from(&why), QString::from(&op)),
        PadEvent::Failed { seq, op } => o.as_mut().failed(seq as i32, QString::from(&op)),
        PadEvent::Ended(why) => o.as_mut().ended(QString::from(&why)),
        PadEvent::Log(_) => {}
    }
}

impl qobject::PadController {
    pub fn attach(self: Pin<&mut Self>) {
        let qt = self.qt_thread();
        global().lock().unwrap().qt = Some(Arc::new(Mutex::new(qt)));
    }

    pub fn apply_pad_on(mut self: Pin<&mut Self>, on: bool) {
        let was = pad_enabled();
        crate::settings::overlay_pref_write("pad-allow", if on { "1" } else { "0" });
        self.as_mut().set_pad_on(on);
        if let Some(n) = node() {
            n.set_pad_on(on);
        }
        if was != on {
            // the `pad1` cap follows at once (not at the 30-min refresh)
            crate::bridge::runtime().spawn(crate::call_ctl::republish_membership());
        }
        bump(self.as_mut());
    }

    pub fn publish_state(self: Pin<&mut Self>, json: &QString) {
        let Some(n) = node() else { return };
        let mut v: Value = serde_json::from_str(&json.to_string()).unwrap_or(Value::Null);
        if let Some(call) = v.get_mut("call").and_then(|c| c.as_object_mut()) {
            let room = call.get("room").and_then(|r| r.as_str()).unwrap_or("").to_string();
            call.insert("gen".into(), json!(gen_for(&room)));
        }
        n.publish_state(&v);
    }

    pub fn claim(self: Pin<&mut Self>, token: i32) -> bool {
        let Some(n) = node() else { return false };
        // the call this device is in RIGHT NOW, from the call layer (not the 250 ms snapshot)
        let live = crate::call_ctl::key_intake_room().map(|(room, _)| {
            let gen = gen_for(&room);
            (room, gen)
        });
        n.claim(token as i64, live)
    }

    pub fn executed(self: Pin<&mut Self>, token: i32) {
        if let Some(n) = node() {
            n.executed(token as i64);
        }
    }

    pub fn refuse_device(self: Pin<&mut Self>, device: &QString) {
        if let Some(n) = node() {
            n.refuse(&device.to_string());
        }
    }

    pub fn refresh_targets(self: Pin<&mut Self>) {
        let Some(n) = node() else { return };
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let rooms: Vec<matrix_sdk::Room> = n.client.joined_rooms().into_iter().filter(crate::rtc::is_call_active).collect();
            let t = n.targets(&rooms).await;
            let (s, from) = (t.to_string(), n.id);
            let _ = qt.queue(move |mut o| {
                // on the Qt thread, together with the guarded mirror: an old session's names never land
                if current_node_id() != Some(from) {
                    return; // the session changed meanwhile
                }
                if let Some(m) = t.get("names").and_then(|x| x.as_object()) {
                    let mut g = names().lock().unwrap();
                    g.clear(); // replaced, not merged: a device signed out disappears
                    for (k, v) in m {
                        if let Some(s) = v.as_str() {
                            g.insert(k.clone(), s.to_string());
                        }
                    }
                }
                if o.as_ref().targets_json().to_string() != s {
                    o.as_mut().set_targets_json(QString::from(&s));
                    bump(o.as_mut());
                }
            });
        });
    }

    pub fn open_pad(mut self: Pin<&mut Self>, room: &QString, device: &QString) {
        let Some(n) = node() else { return };
        n.open_pad(&room.to_string(), &device.to_string());
        self.as_mut().set_remote_json(QString::from(&remote_json(&n)));
        bump(self.as_mut());
    }

    pub fn close_pad(mut self: Pin<&mut Self>) {
        if let Some(n) = node() {
            n.close_pad();
        }
        self.as_mut().set_remote_json(QString::from("null"));
        bump(self.as_mut());
    }

    pub fn set_visible(self: Pin<&mut Self>, vis: bool) {
        if let Some(n) = node() {
            n.set_visible(vis);
        }
    }

    pub fn send(self: Pin<&mut Self>, cmd: &QString) -> i32 {
        let Some(n) = node() else { return 0 };
        let v: Value = serde_json::from_str(&cmd.to_string()).unwrap_or(Value::Null);
        if !v.is_object() {
            return 0;
        }
        n.send(&v) as i32
    }

    pub fn load_sounds(self: Pin<&mut Self>, room: &QString) {
        let Some(n) = node() else { return };
        let qt = self.qt_thread();
        let room = room.to_string();
        crate::bridge::runtime().spawn(async move {
            let list = n.room_sounds(&room).await;
            // who is in that call, by name and avatar (the room list's own lookup: local store + the avatar cache)
            let people = match matrix_sdk::ruma::RoomId::parse(room.as_str()).ok().and_then(|r| n.client.get_room(&r)) {
                Some(r) => crate::nav::resolve_call_participants(&n.client, &r).await,
                None => Vec::new(),
            };
            let (s, from) = (json!({"room": room, "roomSounds": list, "people": people}).to_string(), n.id);
            let _ = qt.queue(move |mut o| {
                if current_node_id() == Some(from) && o.as_ref().sounds_json().to_string() != s {
                    o.as_mut().set_sounds_json(QString::from(&s));
                    bump(o.as_mut());
                }
            });
        });
    }

    pub fn why_text(&self, code: &QString, device: &QString, op: &QString) -> QString {
        QString::from(&crate::padrules::why_text(&code.to_string(), &device.to_string(), &op.to_string()))
    }

    pub fn device_name(&self, device: &QString) -> QString {
        QString::from(&name_of(&device.to_string()))
    }
}
