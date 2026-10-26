//! pad_engine.rs — Pad's two state machines over the pure rules ([`crate::padrules`]), still without any I/O
//! (docs/PAD.md; the web's `features/calls/pad/receiver.ts` + `controller.ts`):
//!   * the RECEIVER (the device in the call): the session table, the reply to every command, the executor plan
//!     (op → the receiver's own button commands, [`exec_plan`]), the push schedule (≤ 4/s, talking-only ≤ 2/s,
//!     none while `vis:false`), the final states (`replaced` / `closed` / `off`, `ended` when the call goes);
//!   * the CONTROLLER (a device driving another of mine): open → ping every 15 s while visible → close, commands
//!     only after the open's answer, the 3 s ack → one resend (same seq) → roll back + "Not reaching", slider pushes
//!     ≤ 4/s with the release always sent, sound taps ≤ 4/s (a newer tap replaces a pending one), `unknown-sound` →
//!     one retry after 2 s as a new command, `no-session` → re-open.
//! Every time is passed in (ms), so the timing is unit-tested with an injected clock. pad_net.rs feeds them real
//! Olm to-device messages and trust facts ([`facts_from`], looked up for every message — nothing is cached);
//! pad_ctl.rs and the QML executor feed them the UI.
//! A command the rules accepted reaches the UI as an execution record: the UI CLAIMS it right before running it
//! ([`Receiver::claim`]: the session still live, Pad on, the device not refused, the very call it was accepted for, ≤
//! 1.5 s old) and says when it ran ([`Receiver::executed`]); the planned effect is applied to the snapshot at once
//! ([`Receiver::on_cmd`]), so the next set sees it (mute then unmute in quick succession ends unmuted).
use crate::padrules::{self as pr, PadCall, PadCmd, PadCtx, PadFacts, PadMe, PadSessions, PadSounds, PadVerdict, PAD_CATS};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicI64, Ordering};

/// Execution tokens are unique in the process (a token of one session's node never matches another's).
static NEXT_TOKEN: AtomicI64 = AtomicI64::new(1);

/// A command handed to the executor must be claimed (and so run) within this long, or it is answered `stale` and
/// never runs; a claimed one that hasn't said "done" by then is answered anyway (its effect shows in the next push).
pub const EXEC_TIMEOUT_MS: i64 = 1_500;
/// Controller: no answer this long after a send → one resend (same seq); this long after the resend → given up.
pub const ACK_MS: i64 = 3_000;
/// Controller: ping period while the Pad is visible.
pub const PING_MS: i64 = 15_000;
/// Controller: slider pushes and sound taps at most every 250 ms (4/s).
pub const PACE_MS: i64 = 250;
/// Controller: `unknown-sound` → one retry this much later (the favourite may not have synced to the PC yet).
pub const SOUND_RETRY_MS: i64 = 2_000;

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}
fn b(v: &Value, k: &str) -> bool {
    v.get(k).and_then(|x| x.as_bool()) == Some(true)
}

// ---- the executor plan --------------------------------------------------------------------------------------

/// One command the receiver's own UI runs — exactly what its button calls (QML `PadHost.run`).
#[derive(Clone, Debug, PartialEq)]
pub enum Exec {
    /// `soundboardPanel.startSound(url, name, emoji)` — the soundboard's play (stop-then-replace, local + call)
    StartSound { url: String },
    /// `soundboardPanel.stopCurrent()`
    StopSound,
    /// `callctl.forceMicMuted(true)` (muting is always allowed)
    MuteMic,
    /// `callctl.toggleMic()` — the mic button's own toggle (keeps the server-mute lock); only planned while muted
    UnmuteMic,
    /// `callctl.applyDeafen(on)`
    Deafen { on: bool },
    /// `afkCtl.setAfk(on)`
    Brb { on: bool },
    /// `audioPrefsCtl.pushVolume(user, cat, level)` — a live slider push (no disk write)
    PushVolume { user: String, cat: &'static str, level: f64 },
    /// `audioPrefsCtl.setVolume(user, cat, level)` — the slider's release
    SetVolume { user: String, cat: &'static str, level: f64 },
    /// `audioPrefsCtl.setMuted(user, cat, muted)` — the row's mute
    SetMuted { user: String, cat: &'static str, muted: bool },
    /// `callctl.leaveCall()`
    Leave,
}

impl Exec {
    /// `{do, …args}` — what QML's `PadHost.run` switches on.
    pub fn to_json(&self) -> Value {
        match self {
            Exec::StartSound { url } => json!({"do": "startSound", "url": url}),
            Exec::StopSound => json!({"do": "stopSound"}),
            Exec::MuteMic => json!({"do": "muteMic"}),
            Exec::UnmuteMic => json!({"do": "unmuteMic"}),
            Exec::Deafen { on } => json!({"do": "deafen", "on": on}),
            Exec::Brb { on } => json!({"do": "brb", "on": on}),
            Exec::PushVolume { user, cat, level } => json!({"do": "pushVolume", "user": user, "cat": cat, "level": level}),
            Exec::SetVolume { user, cat, level } => json!({"do": "setVolume", "user": user, "cat": cat, "level": level}),
            Exec::SetMuted { user, cat, muted } => json!({"do": "setMuted", "user": user, "cat": cat, "muted": muted}),
            Exec::Leave => json!({"do": "leave"}),
        }
    }
}

/// What a verdict runs: nothing unless it is ok AND `run` (every op is a set: no change = nothing to call).
pub fn exec_plan(v: &PadVerdict) -> Vec<Exec> {
    if !v.ok || !v.run {
        return Vec::new();
    }
    match v.cmd.as_ref() {
        Some(PadCmd::SoundPlay { url }) => vec![Exec::StartSound { url: url.clone() }],
        Some(PadCmd::SoundStop) => vec![Exec::StopSound],
        Some(PadCmd::Mic { muted: true }) => vec![Exec::MuteMic],
        Some(PadCmd::Mic { muted: false }) => vec![Exec::UnmuteMic],
        Some(PadCmd::Deafen { on }) => vec![Exec::Deafen { on: *on }],
        Some(PadCmd::Brb { on }) => vec![Exec::Brb { on: *on }],
        Some(PadCmd::Vol { user, cat, level, muted, live }) => {
            if *live {
                return level.map(|l| vec![Exec::PushVolume { user: user.clone(), cat: *cat, level: l }]).unwrap_or_default();
            }
            let mut out = Vec::new();
            if let Some(l) = level {
                out.push(Exec::SetVolume { user: user.clone(), cat: *cat, level: *l });
            }
            if let Some(m) = muted {
                out.push(Exec::SetMuted { user: user.clone(), cat: *cat, muted: *m });
            }
            out
        }
        // the per-person menu's "Reset all to 100%": per category setVolume(1) + setMuted(false), in its order
        Some(PadCmd::VolReset { user }) => PAD_CATS
            .iter()
            .flat_map(|cat| [Exec::SetVolume { user: user.clone(), cat: *cat, level: 1.0 }, Exec::SetMuted { user: user.clone(), cat: *cat, muted: false }])
            .collect(),
        Some(PadCmd::Leave) => vec![Exec::Leave],
        Some(PadCmd::Open { .. }) | Some(PadCmd::Ping { .. }) | Some(PadCmd::Close) | None => Vec::new(),
    }
}

/// A command's planned effect on a snapshot (the flags the rules read for the next set).
fn predict_into(s: &mut Snapshot, cmd: Option<&PadCmd>) {
    match cmd {
        Some(PadCmd::Mic { muted }) => s.me.mic_muted = *muted,
        Some(PadCmd::Deafen { on }) => s.me.deaf = *on,
        Some(PadCmd::Brb { on }) => s.me.brb = *on,
        Some(PadCmd::SoundPlay { url }) => s.sound = url.clone(),
        Some(PadCmd::SoundStop) => s.sound.clear(),
        _ => {}
    }
}

// ---- trust facts (security rules 1–4) from the glue's lookups ---------------------------------------------------

/// What the glue found on record for the claimed sender device (matrix-sdk `Device`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeviceRecord {
    pub device: String,
    /// the device's Curve25519 identity key (base64), "" = none on record
    pub curve: String,
    /// `Device::is_cross_signing_trusted()` (never `is_cross_signed_by_owner`)
    pub cross_signing_trusted: bool,
    /// logged out / deleted on the server
    pub deleted: bool,
}

/// The facts [`pr::check_trust`] decides on, from one to-device event: decrypted by Olm, its Olm sender + the
/// event's `sender`, the Curve25519 key it was encrypted with, the device record found for it (by the SDK's
/// device id, else by that key), my ids, and whether THIS device is verified (all three cross-signing private keys
/// cached). A record whose key differs from the Olm key never binds (rule 3); a deleted device is never trusted.
pub fn facts_from(
    decrypted: bool,
    olm_sender: &str,
    event_sender: &str,
    curve: &str,
    record: Option<&DeviceRecord>,
    my_user: &str,
    my_device: &str,
    self_verified: bool,
) -> PadFacts {
    let (sender_device, curve_matches, peer_trusted) = match record {
        Some(r) if !curve.is_empty() && !r.curve.is_empty() && r.curve == curve => (r.device.clone(), true, r.cross_signing_trusted && !r.deleted),
        Some(r) => (r.device.clone(), false, false),
        None => (String::new(), false, false),
    };
    PadFacts {
        decrypted,
        olm_sender: if decrypted { olm_sender.to_string() } else { String::new() },
        event_sender: event_sender.to_string(),
        sender_device,
        curve_matches,
        my_user_id: my_user.to_string(),
        my_device_id: my_device.to_string(),
        self_verified,
        peer_trusted,
    }
}

// ---- the receiver -------------------------------------------------------------------------------------------

/// The receiver's own call, as its UI publishes it (`padCtl.publishState(json)`, QML PadHost.snapshot):
/// `{call: {room, gen, since} | null, me: {micMuted, ptt, serverMuted, deaf, brb, talk, sound, gate: {allowed, hint}},
///   people: [{u, talk, micOff, away, vol: {cat: n}, muted: {cat: bool}}], recent: [{name, url, emoji}],
///   favourites: [url]}` (`gen` is filled by the glue: the call's join generation).
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub call: Option<(String, String, i64)>,
    pub me: PadMe,
    pub talk: bool,
    pub sound: String,
    pub gate_hint: String,
    pub people: Vec<Value>,
    pub recent: Vec<Value>,
    pub favourites: Vec<String>,
}

impl Snapshot {
    pub fn parse(v: &Value) -> Snapshot {
        let call = v.get("call").filter(|c| c.is_object()).map(|c| {
            (s(c, "room").to_string(), s(c, "gen").to_string(), c.get("since").and_then(|x| x.as_i64()).unwrap_or(0).max(0))
        });
        let me = v.get("me").cloned().unwrap_or(Value::Null);
        let gate = me.get("gate").cloned().unwrap_or(Value::Null);
        let people: Vec<Value> = v.get("people").and_then(|p| p.as_array()).cloned().unwrap_or_default();
        let recent: Vec<Value> = v.get("recent").and_then(|p| p.as_array()).cloned().unwrap_or_default();
        let favourites = v
            .get("favourites")
            .and_then(|p| p.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        Snapshot {
            call,
            me: PadMe {
                mic_muted: b(&me, "micMuted"),
                ptt: b(&me, "ptt"),
                server_muted: b(&me, "serverMuted"),
                deaf: b(&me, "deaf"),
                brb: b(&me, "brb"),
                sound_role: b(&gate, "allowed"),
            },
            talk: b(&me, "talk"),
            sound: s(&me, "sound").to_string(),
            gate_hint: s(&gate, "hint").to_string(),
            people,
            recent,
            favourites,
        }
    }
    /// The state's `me.mic`: a moderator's lock first, then push-to-talk, then muted.
    pub fn mic(&self) -> &'static str {
        if self.me.server_muted {
            "server"
        } else if self.me.ptt {
            "ptt"
        } else if self.me.mic_muted {
            "muted"
        } else {
            "on"
        }
    }
    /// The bare user ids in the call with me (the `vol` / `vol.reset` allow-list).
    pub fn participants(&self) -> Vec<String> {
        self.people.iter().filter_map(|p| p.get("u").and_then(|u| u.as_str()).map(str::to_string)).collect()
    }
    fn recent_urls(&self) -> Vec<String> {
        self.recent.iter().filter_map(|r| r.get("url").and_then(|u| u.as_str()).map(str::to_string)).collect()
    }
    /// `build_state`'s `call` input (None outside a call).
    pub fn call_json(&self) -> Option<Value> {
        let (room, gen, since) = self.call.as_ref()?;
        Some(json!({
            "room": room, "gen": gen, "since": since,
            "me": {"mic": self.mic(), "deaf": self.me.deaf, "brb": self.me.brb, "talk": self.talk, "sound": self.sound,
                   "gate": {"allowed": self.me.sound_role, "hint": self.gate_hint}},
            "people": self.people, "recent": self.recent,
        }))
    }
    /// Everything a state shows except who is talking right now (a change here is a full push).
    fn full_sig(&self) -> String {
        let mut people: Vec<Value> = self.people.clone();
        for p in &mut people {
            if let Some(m) = p.as_object_mut() {
                m.remove("talk");
            }
        }
        json!([self.call, self.mic(), self.me.deaf, self.me.brb, self.sound, self.me.sound_role, self.gate_hint, people, self.recent]).to_string()
    }
    /// Who is talking (a change here alone is a talking-only push).
    fn talk_sig(&self) -> String {
        let t: Vec<bool> = self.people.iter().map(|p| b(p, "talk")).collect();
        json!([self.talk, t]).to_string()
    }
}

/// What the receiver wants done.
#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    /// one `dev.tinhat.pad.state` to exactly this device (the glue re-checks rules 1–4 for it first: rule 10)
    Send { device: String, content: Value },
    /// run these commands on the UI thread, then say `executed(token)`
    Exec { token: i64, cmds: Vec<Exec> },
}

/// A command handed to the executor: what it was accepted for, so the UI can claim it only while that still holds.
#[derive(Clone, Debug)]
struct ExecRec {
    token: i64,
    sid: String,
    device: String,
    seq: i64,
    /// the command expects an answer (every op but a live slider push)
    reply: bool,
    /// the call (room, gen) it was accepted for
    call: Option<(String, String)>,
    at: i64,
    claimed: bool,
    /// its planned effect (kept until it is confirmed or dropped: the effective snapshot = confirmed + these, in order)
    cmd: Option<PadCmd>,
}

/// The receiver: one per signed-in device.
#[derive(Debug)]
pub struct Receiver {
    pub table: PadSessions,
    pad_on: bool,
    /// what the UI last published
    confirmed: Snapshot,
    /// the effective snapshot the rules read: `confirmed` + the planned effects of the commands still pending
    snap: Snapshot,
    rseq: i64,
    last_call: Option<(String, String)>,
    /// sid → (a full change waits, a talking-only change waits)
    dirty: HashMap<String, (bool, bool)>,
    pending: Vec<ExecRec>,
}

impl Receiver {
    pub fn new(pad_on: bool) -> Receiver {
        Receiver { table: PadSessions::default(), pad_on, confirmed: Snapshot::default(), snap: Snapshot::default(), rseq: 0, last_call: None, dirty: HashMap::new(), pending: Vec::new() }
    }
    pub fn pad_on(&self) -> bool {
        self.pad_on
    }
    pub fn snapshot(&self) -> &Snapshot {
        &self.snap
    }
    fn devices(&self) -> HashMap<String, String> {
        self.table.sessions.iter().map(|x| (x.sid.clone(), x.device.clone())).collect()
    }

    /// A state for `sid` (rseq + 1; the first one counts from now_ms × 1000, so a restarted receiver's states stay
    /// above the ones it sent before). `with_call`: the current call (None outside one).
    fn state(&mut self, sid: &str, re: i64, ok: bool, why: &str, with_call: bool, now: i64) -> Value {
        if self.rseq == 0 {
            self.rseq = now.max(0).saturating_mul(1000);
        }
        self.rseq += 1;
        let call = if with_call { self.snap.call_json().unwrap_or(Value::Null) } else { Value::Null };
        pr::build_state(&json!({"sid": sid, "rseq": self.rseq, "rt": now.max(0), "re": re, "ok": ok, "why": why, "call": call})).0
    }
    /// A final state: nothing to control any more, and why.
    fn final_state(&mut self, device: &str, sid: &str, why: &str, now: i64) -> Out {
        self.dirty.remove(sid);
        Out::Send { device: device.to_string(), content: self.state(sid, 0, false, why, false, now) }
    }

    /// An answer carries the call only with Pad on, in a call, and to the asking device's live session (a refused
    /// open, a close, an answer to a device without one: `call: null`).
    fn reply_has_call(&self, sid: &str, device: &str, now: i64) -> bool {
        self.pad_on && self.snap.call.is_some() && self.table.get(sid, now).map(|s| s.device == device).unwrap_or(false)
    }

    /// The PadCtx the rules read: the snapshot + the sounds the glue looked up for a `sound.play` (my favourites
    /// on record, the call room's sounds).
    pub fn ctx(&self, store_favourites: &[String], call_room_sounds: &[String]) -> PadCtx {
        let mut favourites = self.snap.favourites.clone();
        favourites.extend(store_favourites.iter().cloned());
        let mut room_sounds = HashMap::new();
        if let Some((room, _, _)) = &self.snap.call {
            room_sounds.insert(room.clone(), call_room_sounds.to_vec());
        }
        PadCtx {
            pad_on: self.pad_on,
            call: self.snap.call.as_ref().map(|(room, gen, _)| PadCall { room: room.clone(), gen: gen.clone() }),
            participants: self.snap.participants(),
            me: self.snap.me.clone(),
            sounds: PadSounds { favourites, room_sounds, recent: self.snap.recent_urls() },
        }
    }

    /// Settings → Calls → Pad switched. Off: every session ends with a final state `off` (and every later
    /// command is refused `off` by the rules).
    pub fn set_pad_on(&mut self, on: bool, now: i64) -> Vec<Out> {
        self.pad_on = on;
        if on {
            return Vec::new();
        }
        let devs = self.devices();
        self.pending.retain(|p| p.claimed);
        for p in self.pending.iter_mut() {
            p.reply = false;
        }
        self.recompute();
        let gone = self.table.clear(now);
        gone.iter().filter_map(|sid| devs.get(sid).map(|d| (d.clone(), sid.clone()))).map(|(d, sid)| self.final_state(&d, &sid, "off", now)).collect()
    }

    /// A new snapshot from the UI. The call changing (room or generation; leaving = none) clears the × refusals,
    /// answers every command not yet claimed for the old call `other-call` / `not-in-call` (it never runs), and marks
    /// every session for a full push (a new call, or `call: null, why: "ended"`); otherwise a change marks a full or a
    /// talking-only push.
    pub fn set_snapshot(&mut self, snap: Snapshot, now: i64) -> Vec<Out> {
        let key = snap.call.as_ref().map(|(r, g, _)| (r.clone(), g.clone()));
        let mut outs = Vec::new();
        self.confirmed = snap;
        let prev = std::mem::take(&mut self.snap);
        let (full, talk) = if key != self.last_call {
            if self.last_call.is_some() {
                self.table.clear_refused();
            }
            self.last_call = key.clone();
            let why = if key.is_some() { "other-call" } else { "not-in-call" };
            let gone: Vec<ExecRec> = self.pending.iter().filter(|p| !p.claimed).cloned().collect();
            self.pending.retain(|p| p.claimed);
            self.recompute();
            for p in gone {
                outs.extend(self.refusal(&p, why, now));
            }
            (true, false)
        } else {
            self.recompute();
            (self.snap.full_sig() != prev.full_sig(), self.snap.talk_sig() != prev.talk_sig())
        };
        if full || talk {
            for x in self.table.sessions.iter() {
                let e = self.dirty.entry(x.sid.clone()).or_insert((false, false));
                e.0 |= full;
                e.1 |= talk;
            }
        }
        outs
    }

    /// The effective snapshot again: what the UI confirmed + the planned effect of every command still pending, in
    /// order (a command that was confirmed, refused or dropped no longer counts).
    fn recompute(&mut self) {
        let mut s = self.confirmed.clone();
        for p in &self.pending {
            predict_into(&mut s, p.cmd.as_ref());
        }
        self.snap = s;
    }
    /// The answer to a command that will not run (`why`), when it expects one.
    fn refusal(&mut self, p: &ExecRec, why: &str, now: i64) -> Vec<Out> {
        if !p.reply {
            return Vec::new();
        }
        let with_call = self.reply_has_call(&p.sid, &p.device, now);
        let st = self.state(&p.sid, p.seq, false, why, with_call, now);
        self.table.mark_pushed(&p.sid, now);
        vec![Out::Send { device: p.device.clone(), content: st }]
    }
    /// Commands of sessions that ended (replaced, evicted, closed, lapsed, refused): never run, no answer (a final
    /// state went, or the controller is gone).
    fn retire_sids(&mut self, sids: &[String]) {
        self.pending.retain(|p| p.claimed || !sids.contains(&p.sid));
        for p in self.pending.iter_mut().filter(|p| sids.contains(&p.sid)) {
            p.reply = false; // already running: it finishes, but its session got its final state — no answer after it
        }
        for s in sids {
            self.dirty.remove(s);
        }
        self.recompute();
    }

    /// One `dev.tinhat.pad.cmd` that arrived (Olm-decrypted; `facts` from the glue's lookups). Returns the verdict
    /// (for the log) and what to do: the reply (now, or after the executor ran the command), the executor's
    /// commands, the final `replaced` states of sessions an open pushed out.
    pub fn on_cmd(&mut self, content: &Value, facts: &PadFacts, store_favourites: &[String], call_room_sounds: &[String], now: i64) -> (PadVerdict, Vec<Out>) {
        let devs = self.devices();
        let ctx = self.ctx(store_favourites, call_room_sounds);
        let v = pr::check_pad_message(&mut self.table, pr::PAD_CMD, content, facts, &ctx, now);
        let mut outs = Vec::new();
        self.retire_sids(&v.evicted);
        for sid in &v.evicted {
            if let Some(d) = devs.get(sid) {
                outs.push(self.final_state(&d.clone(), sid, "replaced", now));
            }
        }
        if v.ok && matches!(v.cmd, Some(PadCmd::Close)) {
            self.retire_sids(std::slice::from_ref(&v.sid));
        }
        let cmds = exec_plan(&v);
        if !cmds.is_empty() {
            // (1 ..= 2^31 − 1: the token crosses Qt as an int)
            let token = NEXT_TOKEN.fetch_add(1, Ordering::SeqCst) % 2_147_483_646 + 1;
            let call = self.snap.call.as_ref().map(|(r, g, _)| (r.clone(), g.clone()));
            self.pending.push(ExecRec { token, sid: v.sid.clone(), device: facts.sender_device.clone(), seq: v.re, reply: v.reply, call, at: now, claimed: false, cmd: v.cmd.clone() });
            predict_into(&mut self.snap, v.cmd.as_ref());
            outs.push(Out::Exec { token, cmds });
        } else if v.reply {
            let open_refused = !v.ok && content.get("op").and_then(|o| o.as_str()) == Some("open");
            let with_call = !open_refused && self.reply_has_call(&v.sid, &facts.sender_device, now);
            let st = self.state(&v.sid, v.re, v.ok, v.why, with_call, now);
            self.table.mark_pushed(&v.sid, now);
            self.dirty.remove(&v.sid);
            outs.push(Out::Send { device: facts.sender_device.clone(), content: st });
        }
        (v, outs)
    }

    /// The executor is about to run command `token` (the UI thread, right before): may it? Only while the session is
    /// live and of the same device, Pad is on, the device isn't refused, `live` — the call this device is in RIGHT NOW,
    /// from the call layer (not the snapshot) — is the call the command was accepted for, and it is ≤ 1.5 s old.
    /// false = run nothing (the answer `off` / `closed` / `no-session` / `not-in-call` / `other-call` / `stale` goes
    /// out when the command expects one). A token is claimed once.
    pub fn claim(&mut self, token: i64, live: Option<(String, String)>, now: i64) -> (bool, Vec<Out>) {
        let Some(i) = self.pending.iter().position(|p| p.token == token && !p.claimed) else { return (false, Vec::new()) };
        let p = self.pending[i].clone();
        let why = if !self.pad_on {
            "off"
        } else if self.table.refused.contains(&p.device) {
            "closed"
        } else if self.table.get(&p.sid, now).map(|s| s.device != p.device).unwrap_or(true) {
            "no-session"
        } else if now - p.at > EXEC_TIMEOUT_MS || now < p.at {
            "stale"
        } else if live.is_none() {
            "not-in-call"
        } else if live != p.call {
            "other-call"
        } else {
            ""
        };
        if why.is_empty() {
            self.pending[i].claimed = true;
            return (true, Vec::new());
        }
        self.pending.remove(i);
        self.recompute();
        (false, self.refusal(&p, why, now))
    }
    /// The executor ran (claimed) command `token`, and the UI published its snapshot first: the answer goes out now.
    pub fn executed(&mut self, token: i64, now: i64) -> Vec<Out> {
        let Some(i) = self.pending.iter().position(|p| p.token == token && p.claimed) else { return Vec::new() };
        let p = self.pending.remove(i);
        self.recompute();
        self.reply_ok(&p, now)
    }
    fn reply_ok(&mut self, p: &ExecRec, now: i64) -> Vec<Out> {
        if !p.reply {
            return Vec::new();
        }
        let with_call = self.reply_has_call(&p.sid, &p.device, now);
        let st = self.state(&p.sid, p.seq, true, "", with_call, now);
        self.table.mark_pushed(&p.sid, now);
        self.dirty.remove(&p.sid);
        vec![Out::Send { device: p.device.clone(), content: st }]
    }

    /// × on the chip: that device's sessions end (final state `closed`) and it is refused until the call changes.
    pub fn refuse(&mut self, device: &str, now: i64) -> Vec<Out> {
        let devs = self.devices();
        self.pending.retain(|p| p.claimed || p.device != device);
        for p in self.pending.iter_mut().filter(|p| p.device == device) {
            p.reply = false;
        }
        self.recompute();
        let gone = self.table.refuse(device, now);
        gone.iter().filter_map(|sid| devs.get(sid).map(|d| (d.clone(), sid.clone()))).map(|(d, sid)| self.final_state(&d, &sid, "closed", now)).collect()
    }

    /// The live sessions' controller devices (the chip), oldest first, each once.
    pub fn chips(&self, now: i64) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for x in self.table.live(now) {
            if !out.contains(&x.device) {
                out.push(x.device.clone());
            }
        }
        out
    }
    /// Anything for [`Receiver::tick`] to do (sessions or answers waiting).
    pub fn busy(&self) -> bool {
        !self.table.sessions.is_empty() || !self.pending.is_empty()
    }

    /// The clock: answers the executor never confirmed, lapsed sessions, the paced pushes.
    pub fn tick(&mut self, now: i64) -> Vec<Out> {
        let mut outs = Vec::new();
        let late: Vec<ExecRec> = self.pending.iter().filter(|p| now - p.at >= EXEC_TIMEOUT_MS).cloned().collect();
        self.pending.retain(|p| now - p.at < EXEC_TIMEOUT_MS);
        if !late.is_empty() {
            self.recompute();
        }
        for p in late {
            // claimed (so it ran) but not confirmed: answer ok; never claimed: it will never run — `stale`
            if p.claimed {
                outs.extend(self.reply_ok(&p, now));
            } else {
                outs.extend(self.refusal(&p, "stale", now));
            }
        }
        let lapsed = self.table.sweep(now);
        self.retire_sids(&lapsed);
        let live: Vec<(String, String)> = self.table.live(now).iter().map(|x| (x.sid.clone(), x.device.clone())).collect();
        for (sid, device) in live {
            let Some((full, talk)) = self.dirty.get(&sid).copied() else { continue };
            if !full && !talk {
                self.dirty.remove(&sid);
                continue;
            }
            let wait = self.table.push_wait(&sid, now, !full);
            if wait < 0 {
                self.dirty.remove(&sid); // not wanted (hidden, or talking rings nobody asked for): the next answer carries it
                continue;
            }
            if wait > 0 {
                continue;
            }
            let in_call = self.snap.call.is_some();
            let st = if in_call { self.state(&sid, 0, true, "", true, now) } else { self.state(&sid, 0, false, "ended", false, now) };
            self.table.mark_pushed(&sid, now);
            self.dirty.remove(&sid);
            outs.push(Out::Send { device, content: st });
        }
        outs
    }
}

// ---- the controller -----------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reach {
    /// the open is out, no answer yet
    Connecting,
    /// the last message was answered
    Ok,
    /// "Not reaching {device}": a command (or the open) got no answer after its resend
    Unreachable,
}
impl Reach {
    pub fn as_str(&self) -> &'static str {
        match self {
            Reach::Connecting => "connecting",
            Reach::Ok => "ok",
            Reach::Unreachable => "unreachable",
        }
    }
}

/// What the controller wants done / shown.
#[derive(Clone, Debug, PartialEq)]
pub enum CEv {
    /// one `dev.tinhat.pad.cmd` to the receiver device
    Send(Value),
    /// a newer state was accepted (the window re-renders from [`Controller::state`])
    State,
    /// the receiver answered command `seq` (`ok` / `why`): the optimistic press settles or rolls back
    Ack { seq: i64, ok: bool, why: String, op: String },
    /// no answer after the resend: roll the press back, "Not reaching {device}"
    Failed { seq: i64, op: String },
    /// the session `sid` is gone on the receiver (`no-session`): open a new one (a new sid) — for that session only
    Reopen(String),
    /// the session ended there: `replaced` / `closed` / `off`, a refused open's code, or `unreachable`
    Ended(String),
    /// a state was refused (the reason, for the log)
    Dropped(&'static str),
}

#[derive(Clone, Debug)]
struct Sent {
    content: Value,
    op: String,
    at: i64,
    resent: bool,
    /// `sound.play` may be retried once on `unknown-sound`
    retry: bool,
}

/// One Pad session on the controller side (one target device in one call room).
#[derive(Debug)]
pub struct Controller {
    pub room: String,
    pub device: String,
    pub acc: pr::PadCtrlSession,
    seq: i64,
    /// the open was answered ok: commands may go out (they need its `gen`)
    pub opened: bool,
    pub gen: String,
    /// the newest accepted state
    pub state: Option<Value>,
    pub reach: Reach,
    /// the session is over (the code why); nothing more is sent
    pub ended: Option<String>,
    open_sent: Option<Sent>,
    pending: BTreeMap<i64, Sent>,
    visible: bool,
    last_ping: i64,
    /// ONE live-slider budget for all sliders (≤ 4/s): the last live push, and the one waiting (its slider, the push)
    slider_at: i64,
    slider_wait: Option<((String, String), Value)>,
    /// while opening: the newest state that overtook the open's answer (applied right after it), with its facts
    early: Option<(i64, Value, PadFacts)>,
    sound_at: i64,
    /// a paced tap (or a due retry) waiting: the command, and whether an `unknown-sound` may retry it once
    sound_wait: Option<(Value, bool)>,
    retries: Vec<(i64, Value)>,
}

impl Controller {
    /// Open a session on `device` for the call in `room` (`sid` minted by the glue: 16 base64url characters). The
    /// Olm one-time key is claimed by this first message, not by the first tap.
    pub fn open(room: &str, device: &str, sid: &str, now: i64) -> (Controller, Vec<CEv>) {
        let content = json!({"v": pr::PAD_V, "sid": sid, "seq": 1, "ts": now.max(0), "room": room, "op": "open", "want": ["talk"]});
        let c = Controller {
            room: room.to_string(),
            device: device.to_string(),
            acc: pr::PadCtrlSession { sid: sid.to_string(), device: device.to_string(), last_rseq: 0 },
            seq: 1,
            opened: false,
            gen: String::new(),
            state: None,
            reach: Reach::Connecting,
            ended: None,
            open_sent: Some(Sent { content: content.clone(), op: "open".into(), at: now, resent: false, retry: false }),
            pending: BTreeMap::new(),
            visible: true,
            last_ping: now,
            slider_at: i64::MIN / 2,
            slider_wait: None,
            early: None,
            sound_at: i64::MIN / 2,
            sound_wait: None,
            retries: Vec::new(),
        };
        (c, vec![CEv::Send(content)])
    }

    pub fn sid(&self) -> &str {
        &self.acc.sid
    }
    /// The receiver's ops (the controller shows only listed controls).
    pub fn ops(&self) -> Vec<String> {
        self.state
            .as_ref()
            .and_then(|s| s.get("ops"))
            .and_then(|o| o.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    }
    /// Commands in flight: seq → op (the window's pending rings).
    pub fn pending_ops(&self) -> Vec<(i64, String)> {
        self.pending.iter().map(|(k, v)| (*k, v.op.clone())).collect()
    }
    pub fn pending_content(&self, seq: i64) -> Option<&Value> {
        self.pending.get(&seq).map(|p| &p.content)
    }

    fn emit(&mut self, cmd: &Value, now: i64, track: bool, retry: bool) -> (i64, CEv) {
        self.seq += 1;
        let mut m: Map<String, Value> = cmd.as_object().cloned().unwrap_or_default();
        for k in ["v", "sid", "seq", "ts", "room", "gen"] {
            m.remove(k);
        }
        m.insert("v".into(), json!(pr::PAD_V));
        m.insert("sid".into(), json!(self.acc.sid));
        m.insert("seq".into(), json!(self.seq));
        m.insert("ts".into(), json!(now.max(0)));
        m.insert("room".into(), json!(self.room));
        m.insert("gen".into(), json!(self.gen));
        let content = Value::Object(m);
        if track {
            let op = s(&content, "op").to_string();
            self.pending.insert(self.seq, Sent { content: content.clone(), op, at: now, resent: false, retry });
        }
        (self.seq, CEv::Send(content))
    }

    /// Send one command (`{op, …fields}`). Returns its seq (0 = not sent now: before the open's answer, after the
    /// end, or paced — a paced slider push / tap goes out from [`Controller::tick`]).
    pub fn send(&mut self, cmd: &Value, now: i64) -> (i64, Vec<CEv>) {
        if !self.opened || self.ended.is_some() {
            return (0, Vec::new());
        }
        let op = s(cmd, "op");
        if matches!(op, "open" | "ping" | "close") {
            return (0, Vec::new());
        }
        if op == "vol" {
            let key = (s(cmd, "user").to_string(), s(cmd, "cat").to_string());
            if b(cmd, "live") {
                // a live slider push: ≤ 4/s for ALL sliders together, never answered (the newest push waits)
                if now - self.slider_at < PACE_MS {
                    self.slider_wait = Some((key, cmd.clone()));
                    return (0, Vec::new());
                }
                self.slider_at = now;
                let (seq, ev) = self.emit(cmd, now, false, false);
                return (seq, vec![ev]);
            }
            // the release / a mute: always sent; it cancels the waiting push only if that push is its own slider's
            if self.slider_wait.as_ref().map(|(k, _)| *k == key).unwrap_or(false) {
                self.slider_wait = None;
            }
        }
        if op == "sound.stop" {
            self.sound_wait = None; // Stop cancels a waiting tap and a pending retry
        }
        if op.starts_with("sound.") {
            self.no_sound_retries(); // another tap or Stop: no `unknown-sound` retry of an earlier tap, now or later
        }
        if op == "sound.play" && now - self.sound_at < PACE_MS {
            self.sound_wait = Some((cmd.clone(), true)); // ≤ 4 taps/s: a newer tap replaces the one waiting
            return (0, Vec::new());
        }
        if op == "sound.play" {
            self.sound_at = now;
            self.sound_wait = None;
        }
        let (seq, ev) = self.emit(cmd, now, true, op == "sound.play");
        (seq, vec![ev])
    }

    /// The Pad became visible / hidden: a ping at once (`vis:false` stops the receiver's unsolicited pushes).
    pub fn set_visible(&mut self, vis: bool, now: i64) -> Vec<CEv> {
        if self.visible == vis {
            return Vec::new();
        }
        self.visible = vis;
        if !self.opened || self.ended.is_some() {
            return Vec::new();
        }
        self.last_ping = now;
        let (_, ev) = self.emit(&json!({"op": "ping", "vis": vis}), now, vis, false);
        vec![ev]
    }

    /// Close the session (the Pad window closed).
    pub fn close(&mut self, now: i64) -> Vec<CEv> {
        if self.ended.is_some() {
            return Vec::new();
        }
        self.ended = Some("closed-here".into());
        self.pending.clear();
        if !self.opened {
            return Vec::new();
        }
        let (_, ev) = self.emit(&json!({"op": "close"}), now, false, false);
        vec![ev]
    }

    /// One `dev.tinhat.pad.state` that arrived (`facts` from the glue's lookups).
    /// No `unknown-sound` retry for any tap so far: neither a scheduled one nor one an answer still on its way would
    /// bring.
    fn no_sound_retries(&mut self) {
        self.retries.clear();
        for s in self.pending.values_mut() {
            s.retry = false;
        }
    }

    /// Waiting slider pushes, a waiting tap and the `unknown-sound` retry belong to one call: they go when the call
    /// changes (and Stop cancels the sound ones).
    fn drop_deferred(&mut self) {
        self.slider_wait = None;
        self.sound_wait = None;
        self.no_sound_retries();
        self.retries.clear();
    }

    pub fn on_state(&mut self, content: &Value, facts: &PadFacts, now: i64) -> Vec<CEv> {
        if self.ended.is_some() {
            return vec![CEv::Dropped("ended")]; // this session is over (a new one has its own sid)
        }
        let re0 = content.get("re").and_then(|x| x.as_i64()).unwrap_or(0);
        if self.open_sent.is_some() && re0 != 1 {
            // while opening only the open's answer moves the rseq mark; a state that overtook it (a valid newer one)
            // waits and is applied right after the answer
            let mut probe = self.acc.clone();
            if let Err(reason) = pr::accept_state(&mut probe, pr::PAD_STATE, content, facts) {
                return vec![CEv::Dropped(reason)];
            }
            if self.early.as_ref().map(|e| probe.last_rseq > e.0).unwrap_or(true) {
                self.early = Some((probe.last_rseq, content.clone(), facts.clone()));
            }
            return Vec::new();
        }
        if let Err(reason) = pr::accept_state(&mut self.acc, pr::PAD_STATE, content, facts) {
            return vec![CEv::Dropped(reason)];
        }
        let mut evs = vec![CEv::State];
        self.reach = Reach::Ok;
        let re = content.get("re").and_then(|x| x.as_i64()).unwrap_or(0);
        let ok = b(content, "ok");
        let why = s(content, "why").to_string();
        // the session follows the receiver's call (a switch: another room or generation); what waited for the old one goes
        let call = content.get("call").filter(|c| c.is_object()).map(|c| (s(c, "room").to_string(), s(c, "gen").to_string()));
        match call {
            Some((room, gen)) => {
                if room != self.room || gen != self.gen {
                    self.drop_deferred();
                }
                self.room = room;
                self.gen = gen;
            }
            None => self.drop_deferred(),
        }
        self.state = Some(content.clone());
        if re == 1 && self.open_sent.is_some() {
            self.open_sent = None;
            if ok {
                self.opened = true;
                self.last_ping = now;
                if !self.visible {
                    // hidden while the open was out: tell it now, or it pushes until the lease lapses
                    let (_, ev) = self.emit(&json!({"op": "ping", "vis": false}), now, false, false);
                    evs.push(ev);
                }
                if let Some((_, early, f)) = self.early.take() {
                    evs.extend(self.on_state(&early, &f, now).into_iter().filter(|e| *e != CEv::State));
                }
            } else if why == "no-session" {
                self.ended = Some(why.clone());
                evs.push(CEv::Reopen(self.acc.sid.clone()));
            } else {
                self.ended = Some(why.clone());
                evs.push(CEv::Ended(why.clone()));
            }
            return evs;
        }
        if re > 0 {
            if let Some(sent) = self.pending.remove(&re) {
                if !ok && why == "unknown-sound" && sent.retry {
                    let mut cmd = sent.content.clone();
                    if let Some(m) = cmd.as_object_mut() {
                        for k in ["v", "sid", "seq", "ts", "room", "gen"] {
                            m.remove(k);
                        }
                    }
                    self.retries.push((now + SOUND_RETRY_MS, cmd));
                } else {
                    evs.push(CEv::Ack { seq: re, ok, why: why.clone(), op: sent.op.clone() });
                }
                if !ok && why == "no-session" {
                    self.ended = Some(why.clone());
                    evs.push(CEv::Reopen(self.acc.sid.clone()));
                }
            }
            return evs;
        }
        // unsolicited: a final state ends the session (the receiver leaving its call, `ended`, does not: the session
        // survives and the next call brings the controls back)
        if content.get("call").map(|c| c.is_null()).unwrap_or(false) && matches!(why.as_str(), "replaced" | "closed" | "off" | "version") {
            self.ended = Some(why.clone());
            self.pending.clear();
            evs.push(CEv::Ended(why));
        }
        evs
    }

    /// The clock: resends, give-ups, pings, paced slider pushes and taps, the `unknown-sound` retry.
    pub fn tick(&mut self, now: i64) -> Vec<CEv> {
        let mut evs = Vec::new();
        if self.ended.is_some() {
            return evs;
        }
        if let Some(o) = self.open_sent.as_mut() {
            if now - o.at >= ACK_MS {
                if !o.resent {
                    o.resent = true;
                    o.at = now;
                    evs.push(CEv::Send(o.content.clone()));
                } else {
                    // an open that isn't answered after its resend is abandoned: the next try mints a new sid
                    self.open_sent = None;
                    self.reach = Reach::Unreachable;
                    self.ended = Some("unreachable".into());
                    evs.push(CEv::Failed { seq: 1, op: "open".into() });
                    evs.push(CEv::Ended("unreachable".into()));
                    return evs;
                }
            }
            return evs;
        }
        let mut failed: Vec<i64> = Vec::new();
        for (seq, p) in self.pending.iter_mut() {
            if now - p.at >= ACK_MS {
                if !p.resent {
                    p.resent = true;
                    p.at = now;
                    evs.push(CEv::Send(p.content.clone())); // the same seq: a duplicate is dropped there
                } else {
                    failed.push(*seq);
                }
            }
        }
        if !failed.is_empty() {
            // a command (or ping) unanswered after its resend: this sid is abandoned — the next try is a fresh open
            for seq in failed {
                if let Some(p) = self.pending.remove(&seq) {
                    evs.push(CEv::Failed { seq, op: p.op });
                }
            }
            self.reach = Reach::Unreachable;
            self.ended = Some("unreachable".into());
            self.pending.clear();
            self.drop_deferred();
            evs.push(CEv::Ended("unreachable".into()));
            return evs;
        }
        if self.opened && self.visible && now - self.last_ping >= PING_MS {
            self.last_ping = now;
            let (_, ev) = self.emit(&json!({"op": "ping", "vis": true}), now, true, false);
            evs.push(ev);
        }
        if self.slider_wait.is_some() && now - self.slider_at >= PACE_MS {
            if let Some((_, cmd)) = self.slider_wait.take() {
                self.slider_at = now;
                let (_, ev) = self.emit(&cmd, now, false, false);
                evs.push(ev);
            }
        }
        // a due retry becomes the waiting tap (the same ≤ 4/s budget), unless a newer tap already waits
        let (due, later): (Vec<(i64, Value)>, Vec<(i64, Value)>) = std::mem::take(&mut self.retries).into_iter().partition(|(at, _)| now >= *at);
        self.retries = later;
        if let Some((_, cmd)) = due.into_iter().last() {
            if self.sound_wait.is_none() {
                self.sound_wait = Some((cmd, false)); // a new command (new seq), retried once only
            }
        }
        if self.sound_wait.is_some() && now - self.sound_at >= PACE_MS {
            if let Some((cmd, retry)) = self.sound_wait.take() {
                self.sound_at = now;
                let (_, ev) = self.emit(&cmd, now, true, retry);
                evs.push(ev);
            }
        }
        evs
    }
}

/// The room-list bar's order (the web's `callsNewestFirst`, features/pad/order.ts): the rooms of `targets()`'s
/// `rooms` map whose first (= newest, `pad_targets` sorts them) target has the newest `createdTs` first; ties by room
/// id in code-point order. A room with no target is left out. The bar shows the first one this device isn't in.
pub fn calls_newest_first(rooms: &Map<String, Value>) -> Vec<String> {
    let mut v: Vec<(i64, &String)> = rooms
        .iter()
        .filter_map(|(room, ts)| ts.as_array().and_then(|a| a.first()).map(|t| (t.get("createdTs").and_then(Value::as_i64).unwrap_or(0), room)))
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.chars().cmp(b.1.chars())));
    v.into_iter().map(|(_, r)| r.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = "@pad-a:lab.test";
    const PC: &str = "PCDEVICE";
    const PHONE: &str = "PHONEDEV";
    const ROOM: &str = "!call:lab.test";
    const SID: &str = "Qm9vT2JhUmFjZTEy";
    const FAV: &str = "https://sounds.example.org/bruh.mp3";

    fn facts(from: &str) -> PadFacts {
        PadFacts {
            decrypted: true,
            olm_sender: ME.into(),
            event_sender: ME.into(),
            sender_device: from.into(),
            curve_matches: true,
            my_user_id: ME.into(),
            my_device_id: PC.into(),
            self_verified: true,
            peer_trusted: true,
        }
    }
    fn snap(muted: bool) -> Snapshot {
        Snapshot::parse(&json!({
            "call": {"room": ROOM, "gen": "g1", "since": 1000},
            "me": {"micMuted": muted, "ptt": false, "serverMuted": false, "deaf": false, "brb": false, "talk": false, "sound": "",
                   "gate": {"allowed": true, "hint": ""}},
            "people": [{"u": "@bob:lab.test", "talk": false, "micOff": false, "away": false}],
            "recent": [], "favourites": [FAV],
        }))
    }
    fn cmd(seq: i64, ts: i64, op: &str, extra: Value) -> Value {
        let mut m = json!({"v": 1, "sid": SID, "seq": seq, "ts": ts, "room": ROOM, "gen": "g1", "op": op});
        for (k, v) in extra.as_object().unwrap() {
            m[k] = v.clone();
        }
        m
    }
    fn open(r: &mut Receiver, now: i64) -> Vec<Out> {
        r.on_cmd(&json!({"v": 1, "sid": SID, "seq": 1, "ts": now, "room": ROOM, "op": "open", "want": ["talk"]}), &facts(PHONE), &[], &[], now).1
    }
    fn sends(outs: &[Out]) -> Vec<Value> {
        outs.iter().filter_map(|o| if let Out::Send { content, .. } = o { Some(content.clone()) } else { None }).collect()
    }
    fn execs(outs: &[Out]) -> Vec<(i64, Vec<Exec>)> {
        outs.iter().filter_map(|o| if let Out::Exec { token, cmds } = o { Some((*token, cmds.clone())) } else { None }).collect()
    }
    fn receiver() -> Receiver {
        let mut r = Receiver::new(true);
        r.set_snapshot(snap(false), 0);
        r
    }

    // ---- the executor plan: each op → exactly its button's command; a refusal → nothing ----
    fn verdict_for(content: Value, s: Snapshot) -> PadVerdict {
        let mut r = Receiver::new(true);
        r.set_snapshot(s, 0);
        open(&mut r, 1_000);
        r.on_cmd(&content, &facts(PHONE), &[], &[], 1_100).0
    }

    #[test]
    fn exec_plan_maps_each_op_to_its_button() {
        let u = "@bob:lab.test";
        let cases: Vec<(Value, Snapshot, Vec<Exec>)> = vec![
            (cmd(2, 1_100, "sound.play", json!({"url": FAV})), snap(false), vec![Exec::StartSound { url: FAV.into() }]),
            (cmd(2, 1_100, "sound.stop", json!({})), snap(false), vec![Exec::StopSound]),
            (cmd(2, 1_100, "mic", json!({"muted": true})), snap(false), vec![Exec::MuteMic]),
            (cmd(2, 1_100, "mic", json!({"muted": false})), snap(true), vec![Exec::UnmuteMic]),
            (cmd(2, 1_100, "deafen", json!({"on": true})), snap(false), vec![Exec::Deafen { on: true }]),
            (cmd(2, 1_100, "brb", json!({"on": true})), snap(false), vec![Exec::Brb { on: true }]),
            (cmd(2, 1_100, "vol", json!({"user": u, "cat": "mic", "level": 0.3, "live": true})), snap(false), vec![Exec::PushVolume { user: u.into(), cat: "mic", level: 0.3 }]),
            (cmd(2, 1_100, "vol", json!({"user": u, "cat": "mic", "level": 0.3})), snap(false), vec![Exec::SetVolume { user: u.into(), cat: "mic", level: 0.3 }]),
            (cmd(2, 1_100, "vol", json!({"user": u, "cat": "music", "muted": true})), snap(false), vec![Exec::SetMuted { user: u.into(), cat: "music", muted: true }]),
            (
                cmd(2, 1_100, "vol", json!({"user": u, "cat": "screenshare", "level": 2, "muted": false})),
                snap(false),
                vec![Exec::SetVolume { user: u.into(), cat: "screenshare", level: 2.0 }, Exec::SetMuted { user: u.into(), cat: "screenshare", muted: false }],
            ),
            (cmd(2, 1_100, "leave", json!({})), snap(false), vec![Exec::Leave]),
        ];
        for (c, s, want) in cases {
            let v = verdict_for(c.clone(), s);
            assert_eq!(exec_plan(&v), want, "{c}");
        }
        let reset = exec_plan(&verdict_for(cmd(2, 1_100, "vol.reset", json!({"user": u})), snap(false)));
        assert_eq!(reset.len(), 6);
        for cat in PAD_CATS {
            assert!(reset.contains(&Exec::SetVolume { user: u.into(), cat, level: 1.0 }));
            assert!(reset.contains(&Exec::SetMuted { user: u.into(), cat, muted: false }));
        }
    }

    #[test]
    fn exec_plan_runs_nothing_for_a_refusal_or_no_change() {
        let mut ss = snap(false);
        ss.me.server_muted = true;
        // refused: unknown sound, off-role, server-muted unmute, not-here, ptt
        for (c, s) in [
            (cmd(2, 1_100, "sound.play", json!({"url": "https://other.example/x.mp3"})), snap(false)),
            (cmd(2, 1_100, "mic", json!({"muted": false})), ss.clone()),
            (cmd(2, 1_100, "vol", json!({"user": "@stranger:lab.test", "cat": "mic", "level": 0.5})), snap(false)),
        ] {
            let v = verdict_for(c.clone(), s);
            assert!(!v.ok, "{c}");
            assert!(exec_plan(&v).is_empty(), "{c}");
        }
        let mut no_role = snap(false);
        no_role.me.sound_role = false;
        assert_eq!(verdict_for(cmd(2, 1_100, "sound.play", json!({"url": FAV})), no_role).why, "off-role");
        let mut ptt = snap(true);
        ptt.me.ptt = true;
        assert_eq!(verdict_for(cmd(2, 1_100, "mic", json!({"muted": false})), ptt).why, "ptt");
        // ok but nothing to change (every op is a set): mute while muted, deafen off while hearing
        for (c, s) in [(cmd(2, 1_100, "mic", json!({"muted": true})), snap(true)), (cmd(2, 1_100, "deafen", json!({"on": false})), snap(false))] {
            let v = verdict_for(c.clone(), s);
            assert!(v.ok && !v.run, "{c}");
            assert!(exec_plan(&v).is_empty());
        }
        // ping / open / close never run anything
        assert!(exec_plan(&verdict_for(cmd(2, 1_100, "ping", json!({"vis": true})), snap(false))).is_empty());
    }

    // ---- the receiver ----
    #[test]
    fn open_is_answered_with_the_call() {
        let mut r = receiver();
        let outs = open(&mut r, 1_000);
        let st = sends(&outs);
        assert_eq!(st.len(), 1);
        assert_eq!(st[0]["re"], 1);
        assert_eq!(st[0]["ok"], true);
        assert_eq!(st[0]["call"]["room"], ROOM);
        assert_eq!(st[0]["call"]["gen"], "g1");
        assert_eq!(r.chips(1_000), vec![PHONE.to_string()]);
    }

    #[test]
    fn a_refused_open_carries_no_call() {
        let mut r = Receiver::new(true);
        r.set_snapshot(Snapshot::default(), 0); // not in a call
        let st = sends(&open(&mut r, 1_000));
        assert_eq!(st[0]["ok"], false);
        assert_eq!(st[0]["why"], "not-in-call");
        assert!(st[0]["call"].is_null());
        let mut off = Receiver::new(false);
        off.set_snapshot(snap(false), 0);
        let st = sends(&open(&mut off, 1_000));
        assert_eq!(st[0]["why"], "off");
        assert!(st[0]["call"].is_null());
    }

    #[test]
    fn a_command_runs_then_is_answered_after_the_executor() {
        let mut r = receiver();
        open(&mut r, 1_000);
        let (v, outs) = r.on_cmd(&cmd(2, 1_100, "sound.play", json!({"url": FAV})), &facts(PHONE), &[], &[], 1_100);
        assert!(v.ok && v.run);
        assert!(sends(&outs).is_empty(), "no answer before the executor ran it");
        let ex = execs(&outs);
        assert_eq!(ex.len(), 1);
        assert_eq!(ex[0].1, vec![Exec::StartSound { url: FAV.into() }]);
        assert!(r.executed(ex[0].0, 1_140).is_empty(), "never answered before it was claimed");
        let (ok, none) = r.claim(ex[0].0, Some((ROOM.into(), "g1".into())), 1_145);
        assert!(ok && none.is_empty());
        assert!(!r.claim(ex[0].0, Some((ROOM.into(), "g1".into())), 1_146).0, "claimed once");
        let mut s2 = snap(false);
        s2.sound = FAV.into();
        r.set_snapshot(s2, 1_150);
        let st = sends(&r.executed(ex[0].0, 1_160));
        assert_eq!(st.len(), 1);
        assert_eq!(st[0]["re"], 2);
        assert_eq!(st[0]["ok"], true);
        assert_eq!(st[0]["call"]["me"]["sound"]["url"], FAV, "the answer shows the effect");
        assert!(r.executed(ex[0].0, 1_170).is_empty(), "answered once");
    }

    #[test]
    fn a_published_snapshot_keeps_the_later_planned_effects() {
        // mute A, unmute B queued; A runs and its snapshot (muted) is published; then mute C: it must run (B is still
        // planned, so the effective mic is live)
        let mut r = receiver();
        open(&mut r, 1_000);
        let (_, a) = r.on_cmd(&cmd(2, 1_100, "mic", json!({"muted": true})), &facts(PHONE), &[], &[], 1_100);
        let (_, _) = r.on_cmd(&cmd(3, 1_110, "mic", json!({"muted": false})), &facts(PHONE), &[], &[], 1_110);
        let ta = execs(&a)[0].0;
        assert!(r.claim(ta, Some((ROOM.into(), "g1".into())), 1_120).0);
        r.set_snapshot(snap(true), 1_125); // A ran: the UI publishes muted
        r.executed(ta, 1_126);
        let (v, c) = r.on_cmd(&cmd(4, 1_130, "mic", json!({"muted": true})), &facts(PHONE), &[], &[], 1_130);
        assert!(v.ok && v.run, "C runs after B: {v:?}");
        assert_eq!(execs(&c)[0].1, vec![Exec::MuteMic]);
        // a command dropped before it ran no longer counts
        let mut r = receiver();
        open(&mut r, 1_000);
        let (_, a) = r.on_cmd(&cmd(2, 1_100, "deafen", json!({"on": true})), &facts(PHONE), &[], &[], 1_100);
        let (ok, _) = r.claim(execs(&a)[0].0, None, 1_110); // the call is gone: refused, never runs
        assert!(!ok);
        let (v, _) = r.on_cmd(&cmd(3, 1_120, "deafen", json!({"on": true})), &facts(PHONE), &[], &[], 1_120);
        assert!(v.run, "deafen is still off: this one runs");
    }

    #[test]
    fn a_retired_running_command_stays_silent() {
        let mut r = receiver();
        open(&mut r, 1_000);
        let (_, a) = r.on_cmd(&cmd(2, 1_100, "brb", json!({"on": true})), &facts(PHONE), &[], &[], 1_100);
        let ta = execs(&a)[0].0;
        assert!(r.claim(ta, Some((ROOM.into(), "g1".into())), 1_110).0);
        let fin = sends(&r.refuse(PHONE, 1_120));
        assert_eq!(fin[0]["why"], "closed");
        assert!(r.executed(ta, 1_130).is_empty(), "no answer after the final state");
    }

    #[test]
    fn a_claimed_but_unconfirmed_command_is_answered_after_the_timeout() {
        let mut r = receiver();
        open(&mut r, 1_000);
        let (_, outs) = r.on_cmd(&cmd(2, 1_100, "brb", json!({"on": true})), &facts(PHONE), &[], &[], 1_100);
        let ex = execs(&outs);
        assert!(r.claim(ex[0].0, Some((ROOM.into(), "g1".into())), 1_200).0);
        assert!(sends(&r.tick(1_100 + EXEC_TIMEOUT_MS - 1)).iter().all(|s| s["re"] != 2));
        let st = sends(&r.tick(1_100 + EXEC_TIMEOUT_MS));
        assert!(st.iter().any(|s| s["re"] == 2 && s["ok"] == true));
    }

    #[test]
    fn a_command_never_claimed_never_runs_and_is_answered_stale() {
        let mut r = receiver();
        open(&mut r, 1_000);
        let (_, outs) = r.on_cmd(&cmd(2, 1_100, "leave", json!({})), &facts(PHONE), &[], &[], 1_100);
        let ex = execs(&outs);
        let st = sends(&r.tick(1_100 + EXEC_TIMEOUT_MS));
        assert!(st.iter().any(|s| s["re"] == 2 && s["ok"] == false && s["why"] == "stale"));
        assert!(!r.claim(ex[0].0, Some((ROOM.into(), "g1".into())), 2_700).0, "too late: it must not run now");
    }

    #[test]
    fn a_claim_checks_the_live_call_and_the_session() {
        let live = || Some((ROOM.to_string(), "g1".to_string()));
        // the call layer is already in another call (the snapshot hasn't caught up): refused other-call, nothing runs
        let mut r = receiver();
        open(&mut r, 1_000);
        let (_, outs) = r.on_cmd(&cmd(2, 1_100, "leave", json!({})), &facts(PHONE), &[], &[], 1_100);
        let tok = execs(&outs)[0].0;
        let (ok, outs) = r.claim(tok, Some(("!other:lab.test".into(), "g9".into())), 1_150);
        assert!(!ok);
        assert_eq!(sends(&outs)[0]["why"], "other-call");
        // not in a call any more
        let mut r = receiver();
        open(&mut r, 1_000);
        let (_, outs) = r.on_cmd(&cmd(2, 1_100, "leave", json!({})), &facts(PHONE), &[], &[], 1_100);
        let (ok, outs) = r.claim(execs(&outs)[0].0, None, 1_150);
        assert!(!ok);
        assert_eq!(sends(&outs)[0]["why"], "not-in-call");
        // the snapshot's call changed before the UI ran it: answered other-call there and then
        let mut r = receiver();
        open(&mut r, 1_000);
        let (_, outs) = r.on_cmd(&cmd(2, 1_100, "leave", json!({})), &facts(PHONE), &[], &[], 1_100);
        let tok = execs(&outs)[0].0;
        let mut s2 = snap(false);
        s2.call = Some((ROOM.into(), "g2".into(), 2_000));
        let st = sends(&r.set_snapshot(s2, 1_120));
        assert!(st.iter().any(|s| s["re"] == 2 && s["why"] == "other-call"));
        assert!(!r.claim(tok, Some((ROOM.into(), "g2".into())), 1_130).0);
        // × on the chip, Pad off, the session replaced: never runs
        for end in 0..3 {
            let mut r = receiver();
            open(&mut r, 1_000);
            let (_, outs) = r.on_cmd(&cmd(2, 1_100, "leave", json!({})), &facts(PHONE), &[], &[], 1_100);
            let tok = execs(&outs)[0].0;
            match end {
                0 => drop(r.refuse(PHONE, 1_110)),
                1 => drop(r.set_pad_on(false, 1_110)),
                _ => drop(r.on_cmd(&json!({"v": 1, "sid": "Qm9vT2JhUmFjZTEz", "seq": 1, "ts": 1_110, "room": ROOM, "op": "open"}), &facts(PHONE), &[], &[], 1_110)),
            }
            assert!(!r.claim(tok, live(), 1_120).0, "end {end}");
        }
    }

    #[test]
    fn consecutive_sets_see_the_planned_effect() {
        // mute then unmute before the UI ran the mute: the unmute must run too (the end state is unmuted)
        let mut r = receiver();
        open(&mut r, 1_000);
        let (_, a) = r.on_cmd(&cmd(2, 1_100, "mic", json!({"muted": true})), &facts(PHONE), &[], &[], 1_100);
        let (v, b) = r.on_cmd(&cmd(3, 1_110, "mic", json!({"muted": false})), &facts(PHONE), &[], &[], 1_110);
        assert_eq!(execs(&a)[0].1, vec![Exec::MuteMic]);
        assert!(v.ok && v.run);
        assert_eq!(execs(&b)[0].1, vec![Exec::UnmuteMic]);
        // deafen on, then on again: the second is a no-op
        let (_, _) = r.on_cmd(&cmd(4, 1_120, "deafen", json!({"on": true})), &facts(PHONE), &[], &[], 1_120);
        let (v, _) = r.on_cmd(&cmd(5, 1_130, "deafen", json!({"on": true})), &facts(PHONE), &[], &[], 1_130);
        assert!(v.ok && !v.run);
    }

    #[test]
    fn a_replayed_seq_runs_once() {
        let mut r = receiver();
        open(&mut r, 1_000);
        let c = cmd(2, 1_100, "sound.play", json!({"url": FAV}));
        let (_, first) = r.on_cmd(&c, &facts(PHONE), &[], &[], 1_100);
        let (v, again) = r.on_cmd(&c, &facts(PHONE), &[], &[], 1_200);
        assert_eq!(execs(&first).len(), 1);
        assert!(again.is_empty(), "dropped silently");
        assert_eq!(v.reason, "old-seq");
    }

    #[test]
    fn a_refusal_is_answered_and_runs_nothing() {
        let mut r = receiver();
        open(&mut r, 1_000);
        let (v, outs) = r.on_cmd(&cmd(2, 1_100, "sound.play", json!({"url": "https://other.example/x.mp3"})), &facts(PHONE), &[], &[], 1_100);
        assert_eq!(v.why, "unknown-sound");
        assert!(execs(&outs).is_empty());
        let st = sends(&outs);
        assert_eq!(st[0]["re"], 2);
        assert_eq!(st[0]["why"], "unknown-sound");
        assert_eq!(st[0]["call"]["room"], ROOM, "a refusal still shows the call");
        // the store's lists count too (a favourite the UI hasn't re-read yet, the call room's sounds)
        let room_sound = "https://sounds.example.org/room.mp3";
        let (v, _) = r.on_cmd(&cmd(3, 1_200, "sound.play", json!({"url": room_sound})), &facts(PHONE), &[], &[room_sound.to_string()], 1_200);
        assert!(v.ok && v.run);
    }

    #[test]
    fn a_live_slider_push_runs_and_is_never_answered() {
        let mut r = receiver();
        open(&mut r, 1_000);
        let (_, outs) = r.on_cmd(&cmd(2, 1_100, "vol", json!({"user": "@bob:lab.test", "cat": "mic", "level": 0.4, "live": true})), &facts(PHONE), &[], &[], 1_100);
        let ex = execs(&outs);
        assert_eq!(ex.len(), 1);
        assert!(r.executed(ex[0].0, 1_110).is_empty(), "no answer for a live push");
        assert!(sends(&r.tick(1_100 + EXEC_TIMEOUT_MS + 1)).iter().all(|s| s["re"] != 2));
    }

    #[test]
    fn pushes_are_paced_and_talking_rings_only_when_asked() {
        let mut r = receiver();
        open(&mut r, 1_000); // the answer at 1_000 counts as a push
        let mut s2 = snap(true);
        r.set_snapshot(s2.clone(), 1_050);
        assert!(sends(&r.tick(1_100)).is_empty(), "≤ 4/s: 250 ms after the last state");
        let st = sends(&r.tick(1_250));
        assert_eq!(st.len(), 1);
        assert_eq!(st[0]["re"], 0);
        assert_eq!(st[0]["call"]["me"]["mic"], "muted");
        // talking only: ≤ 2/s
        s2.people[0]["talk"] = json!(true);
        r.set_snapshot(s2.clone(), 1_300);
        assert!(sends(&r.tick(1_600)).is_empty(), "talking-only waits 500 ms");
        assert_eq!(sends(&r.tick(1_750)).len(), 1);
        // a controller that didn't ask for talk gets no talking-only pushes
        let mut r2 = receiver();
        r2.on_cmd(&json!({"v": 1, "sid": SID, "seq": 1, "ts": 1_000, "room": ROOM, "op": "open"}), &facts(PHONE), &[], &[], 1_000);
        let mut t = snap(false);
        t.people[0]["talk"] = json!(true);
        r2.set_snapshot(t, 1_100);
        assert!(sends(&r2.tick(5_000)).is_empty());
    }

    #[test]
    fn nothing_is_pushed_while_hidden() {
        let mut r = receiver();
        open(&mut r, 1_000);
        r.on_cmd(&cmd(2, 1_100, "ping", json!({"vis": false})), &facts(PHONE), &[], &[], 1_100);
        r.set_snapshot(snap(true), 1_200);
        assert!(sends(&r.tick(3_000)).is_empty());
        // visible again: the ping's answer carries the change
        let (_, outs) = r.on_cmd(&cmd(3, 3_100, "ping", json!({"vis": true})), &facts(PHONE), &[], &[], 3_100);
        assert_eq!(sends(&outs)[0]["call"]["me"]["mic"], "muted");
    }

    #[test]
    fn leaving_the_call_pushes_ended_and_clears_refusals() {
        let mut r = receiver();
        open(&mut r, 1_000);
        r.set_snapshot(Snapshot::default(), 2_000);
        let st = sends(&r.tick(2_000));
        assert_eq!(st.len(), 1);
        assert!(st[0]["call"].is_null());
        assert_eq!(st[0]["why"], "ended");
        // × refuses a device until the call changes
        let mut r = receiver();
        open(&mut r, 1_000);
        let fin = sends(&r.refuse(PHONE, 1_500));
        assert_eq!(fin[0]["why"], "closed");
        assert!(fin[0]["call"].is_null());
        assert!(r.chips(1_500).is_empty());
        let (v, _) = r.on_cmd(&json!({"v": 1, "sid": "Qm9vT2JhUmFjZTEz", "seq": 1, "ts": 1_600, "room": ROOM, "op": "open"}), &facts(PHONE), &[], &[], 1_600);
        assert_eq!(v.why, "closed");
        let mut s2 = snap(false);
        s2.call = Some((ROOM.into(), "g2".into(), 2_000)); // rejoined: a new generation
        r.set_snapshot(s2, 2_000);
        let (v, _) = r.on_cmd(&json!({"v": 1, "sid": "Qm9vT2JhUmFjZTE0", "seq": 1, "ts": 2_100, "room": ROOM, "op": "open"}), &facts(PHONE), &[], &[], 2_100);
        assert!(v.ok, "{v:?}");
    }

    #[test]
    fn pad_off_ends_every_session() {
        let mut r = receiver();
        open(&mut r, 1_000);
        let fin = sends(&r.set_pad_on(false, 1_500));
        assert_eq!(fin.len(), 1);
        assert_eq!(fin[0]["why"], "off");
        assert!(r.chips(1_500).is_empty());
    }

    #[test]
    fn a_second_open_from_the_same_device_replaces_the_first() {
        let mut r = receiver();
        open(&mut r, 1_000);
        let (_, outs) = r.on_cmd(&json!({"v": 1, "sid": "Qm9vT2JhUmFjZTEz", "seq": 1, "ts": 1_100, "room": ROOM, "op": "open"}), &facts(PHONE), &[], &[], 1_100);
        let st = sends(&outs);
        assert_eq!(st.len(), 2);
        assert_eq!(st[0]["sid"], SID);
        assert_eq!(st[0]["why"], "replaced");
        assert_eq!(st[1]["sid"], "Qm9vT2JhUmFjZTEz");
        assert_eq!(st[1]["ok"], true);
    }

    #[test]
    fn answers_carry_the_call_only_for_the_asking_session() {
        let mut r = receiver();
        open(&mut r, 1_000);
        // close: answered, without the call
        let (_, outs) = r.on_cmd(&cmd(2, 1_100, "close", json!({})), &facts(PHONE), &[], &[], 1_100);
        let st = sends(&outs);
        assert_eq!(st[0]["ok"], true);
        assert!(st[0]["call"].is_null());
        // a refusal to a device without a live session: no call
        let (_, outs) = r.on_cmd(&json!({"v": 1, "sid": "Qm9vT2JhUmFjZTE5", "seq": 1, "ts": 1_200, "room": ROOM, "gen": "g1", "op": "ping", "vis": true}), &facts(PHONE), &[], &[], 1_200);
        let st = sends(&outs);
        assert_eq!(st[0]["why"], "no-session");
        assert!(st[0]["call"].is_null());
        // Pad off: refused `off`, no call
        let mut r = receiver();
        open(&mut r, 1_000);
        r.set_pad_on(false, 1_050);
        let (_, outs) = r.on_cmd(&cmd(2, 1_100, "mic", json!({"muted": true})), &facts(PHONE), &[], &[], 1_100);
        let st = sends(&outs);
        assert_eq!(st[0]["why"], "off");
        assert!(st[0]["call"].is_null());
    }

    #[test]
    fn rseq_counts_from_now_times_1000() {
        let mut r = receiver();
        let a = sends(&open(&mut r, 1_761_400_000_000))[0]["rseq"].as_i64().unwrap();
        assert!(a > 1_761_400_000_000 * 1000);
        let mut r2 = receiver();
        let b = sends(&open(&mut r2, 1_761_400_009_000))[0]["rseq"].as_i64().unwrap();
        assert!(b > a, "a restarted receiver counts above its old states");
    }

    #[test]
    fn rseq_grows_with_every_state() {
        let mut r = receiver();
        let a = sends(&open(&mut r, 1_000))[0]["rseq"].as_i64().unwrap();
        let (_, outs) = r.on_cmd(&cmd(2, 1_100, "ping", json!({"vis": true})), &facts(PHONE), &[], &[], 1_100);
        let b = sends(&outs)[0]["rseq"].as_i64().unwrap();
        assert!(b > a);
    }

    // ---- trust glue (stubbed lookups) ----
    #[test]
    fn facts_from_binds_only_a_matching_curve_and_trusts_only_cross_signing() {
        let rec = DeviceRecord { device: PHONE.into(), curve: "CURVE".into(), cross_signing_trusted: true, deleted: false };
        let f = facts_from(true, ME, ME, "CURVE", Some(&rec), ME, PC, true);
        assert_eq!(pr::check_trust(&f), Ok(()));
        let f = facts_from(true, ME, ME, "OTHER", Some(&rec), ME, PC, true);
        assert_eq!(pr::check_trust(&f), Err("unbound"));
        let f = facts_from(true, ME, ME, "CURVE", None, ME, PC, true);
        assert_eq!(pr::check_trust(&f), Err("unbound"));
        let untrusted = DeviceRecord { cross_signing_trusted: false, ..rec.clone() };
        assert_eq!(pr::check_trust(&facts_from(true, ME, ME, "CURVE", Some(&untrusted), ME, PC, true)), Err("peer-untrusted"));
        let deleted = DeviceRecord { deleted: true, ..rec.clone() };
        assert_eq!(pr::check_trust(&facts_from(true, ME, ME, "CURVE", Some(&deleted), ME, PC, true)), Err("peer-untrusted"));
        assert_eq!(pr::check_trust(&facts_from(true, ME, ME, "CURVE", Some(&rec), ME, PC, false)), Err("self-unverified"));
        assert_eq!(pr::check_trust(&facts_from(false, ME, ME, "CURVE", Some(&rec), ME, PC, true)), Err("not-olm"));
        assert_eq!(pr::check_trust(&facts_from(true, "@eve:lab.test", "@eve:lab.test", "CURVE", Some(&rec), ME, PC, true)), Err("other-user"));
        assert_eq!(pr::check_trust(&facts_from(true, "@eve:lab.test", ME, "CURVE", Some(&rec), ME, PC, true)), Err("sender-mismatch"));
        let own = DeviceRecord { device: PC.into(), ..rec };
        assert_eq!(pr::check_trust(&facts_from(true, ME, ME, "CURVE", Some(&own), ME, PC, true)), Err("own-device"));
    }

    // ---- the controller (injected clock) ----
    fn state(sid: &str, rseq: i64, re: i64, ok: bool, why: &str, call: bool) -> Value {
        let c = if call {
            json!({"room": ROOM, "gen": "g1", "since": 0, "me": {"mic": "on", "deaf": false, "brb": false, "talk": false, "sound": null,
                   "gate": {"allowed": true, "hint": ""}}, "people": [], "more": 0, "recent": []})
        } else {
            Value::Null
        };
        json!({"v": 1, "sid": sid, "rseq": rseq, "rt": 1, "re": re, "ok": ok, "why": why, "ops": pr::PAD_OPS, "call": c})
    }
    fn pc_facts() -> PadFacts {
        PadFacts { my_device_id: PHONE.into(), sender_device: PC.into(), ..facts(PC) }
    }
    fn sent(evs: &[CEv]) -> Vec<Value> {
        evs.iter().filter_map(|e| if let CEv::Send(v) = e { Some(v.clone()) } else { None }).collect()
    }
    fn opened(now: i64) -> Controller {
        let (mut c, _) = Controller::open(ROOM, PC, SID, now);
        c.on_state(&state(SID, 1, 1, true, "", true), &pc_facts(), now + 50);
        assert!(c.opened);
        c
    }

    #[test]
    fn no_command_before_the_open_is_answered() {
        let (mut c, evs) = Controller::open(ROOM, PC, SID, 0);
        assert_eq!(sent(&evs)[0]["op"], "open");
        assert_eq!(sent(&evs)[0]["seq"], 1);
        assert_eq!(c.send(&json!({"op": "mic", "muted": true}), 10).0, 0);
        c.on_state(&state(SID, 1, 1, true, "", true), &pc_facts(), 100);
        let (seq, evs) = c.send(&json!({"op": "mic", "muted": true}), 110);
        assert_eq!(seq, 2);
        let m = &sent(&evs)[0];
        assert_eq!(m["gen"], "g1", "the open's gen");
        assert_eq!(m["sid"], SID);
        assert_eq!(m["ts"], 110);
    }

    #[test]
    fn an_unanswered_open_is_resent_once_then_abandoned() {
        let (mut c, _) = Controller::open(ROOM, PC, SID, 0);
        assert!(sent(&c.tick(ACK_MS - 1)).is_empty());
        let again = sent(&c.tick(ACK_MS));
        assert_eq!(again.len(), 1);
        assert_eq!(again[0]["seq"], 1, "the same seq");
        assert_eq!(again[0]["ts"], 0, "the same message");
        let evs = c.tick(2 * ACK_MS);
        assert!(evs.contains(&CEv::Failed { seq: 1, op: "open".into() }));
        assert!(evs.contains(&CEv::Ended("unreachable".into())));
        assert_eq!(c.reach, Reach::Unreachable);
        assert!(sent(&c.tick(10 * ACK_MS)).is_empty(), "abandoned: a new try mints a new sid");
    }

    #[test]
    fn no_ack_means_one_resend_then_a_roll_back() {
        let mut c = opened(0);
        let (seq, _) = c.send(&json!({"op": "deafen", "on": true}), 1_000);
        assert!(sent(&c.tick(1_000 + ACK_MS - 1)).is_empty());
        let r = sent(&c.tick(1_000 + ACK_MS));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["seq"], seq);
        let evs = c.tick(1_000 + 2 * ACK_MS);
        assert!(evs.contains(&CEv::Failed { seq, op: "deafen".into() }));
        assert!(evs.contains(&CEv::Ended("unreachable".into())), "the sid is abandoned");
        assert_eq!(c.reach, Reach::Unreachable);
        // nothing more on it: the next try is a fresh open (a new sid)
        assert_eq!(c.send(&json!({"op": "brb", "on": true}), 8_000).0, 0);
        assert_eq!(c.on_state(&state(SID, 2, 0, true, "", true), &pc_facts(), 9_000), vec![CEv::Dropped("ended")]);
    }

    #[test]
    fn an_unanswered_ping_abandons_the_session_too() {
        let mut c = opened(0);
        let p = sent(&c.tick(50 + PING_MS));
        assert_eq!(p[0]["op"], "ping");
        assert!(!sent(&c.tick(50 + PING_MS + ACK_MS)).is_empty(), "one resend");
        assert!(c.tick(50 + PING_MS + 2 * ACK_MS).contains(&CEv::Ended("unreachable".into())));
    }

    #[test]
    fn one_slider_budget_for_all_sliders() {
        let mut c = opened(0);
        let live = |u: &str, l: f64| json!({"op": "vol", "user": u, "cat": "mic", "level": l, "live": true});
        assert!(c.send(&live("@a:lab.test", 0.5), 1_000).0 > 0);
        assert_eq!(c.send(&live("@b:lab.test", 0.5), 1_100).0, 0, "another slider shares the budget");
        // a's release doesn't cancel b's waiting push; b's own release would
        c.send(&json!({"op": "vol", "user": "@a:lab.test", "cat": "mic", "level": 0.5}), 1_150);
        let out = sent(&c.tick(1_250));
        assert!(out.iter().any(|m| m["user"] == "@b:lab.test" && m["live"] == true));
        assert_eq!(c.send(&live("@b:lab.test", 0.6), 1_300).0, 0);
        c.send(&json!({"op": "vol", "user": "@b:lab.test", "cat": "mic", "level": 0.6}), 1_320);
        assert!(sent(&c.tick(1_600)).iter().all(|m| m["live"] != true), "its own release cancels it");
    }

    #[test]
    fn a_state_overtaking_the_open_answer_waits_for_it() {
        let (mut c, _) = Controller::open(ROOM, PC, SID, 0);
        // the push (rseq 6) arrives before the open's answer (rseq 5)
        let mut push = state(SID, 6, 0, true, "", true);
        push["call"]["me"]["mic"] = json!("muted");
        assert!(c.on_state(&push, &pc_facts(), 50).is_empty());
        let evs = c.on_state(&state(SID, 5, 1, true, "", true), &pc_facts(), 60);
        assert!(c.opened, "the answer still opens it: {evs:?}");
        assert_eq!(c.acc.last_rseq, 6, "the newer push applied right after");
        assert_eq!(c.state.as_ref().unwrap()["call"]["me"]["mic"], "muted");
    }

    #[test]
    fn stop_before_the_answer_means_no_retry() {
        let mut c = opened(0);
        let (seq, _) = c.send(&json!({"op": "sound.play", "url": FAV}), 1_000);
        c.send(&json!({"op": "sound.stop"}), 1_300);
        c.on_state(&state(SID, 2, seq, false, "unknown-sound", true), &pc_facts(), 1_400);
        assert!(sent(&c.tick(5_000)).iter().all(|m| m["op"] != "sound.play"), "Stop came first: no retry");
    }

    #[test]
    fn another_tap_drops_the_unknown_sound_retry() {
        let mut c = opened(0);
        let (seq, _) = c.send(&json!({"op": "sound.play", "url": FAV}), 1_000);
        c.on_state(&state(SID, 2, seq, false, "unknown-sound", true), &pc_facts(), 1_100);
        c.send(&json!({"op": "sound.play", "url": "https://sounds.example.org/other.mp3"}), 1_500);
        assert!(sent(&c.tick(5_000)).iter().all(|m| m["url"] != FAV));
    }

    #[test]
    fn an_ack_settles_the_press() {
        let mut c = opened(0);
        let (seq, _) = c.send(&json!({"op": "brb", "on": true}), 1_000);
        let evs = c.on_state(&state(SID, 2, seq, true, "", true), &pc_facts(), 1_200);
        assert!(evs.contains(&CEv::Ack { seq, ok: true, why: String::new(), op: "brb".into() }));
        assert!(c.pending_ops().is_empty());
        assert!(sent(&c.tick(1_000 + 3 * ACK_MS)).iter().all(|m| m["op"] != "brb"), "no resend after the ack");
        // an older rseq is ignored
        assert_eq!(c.on_state(&state(SID, 2, 0, true, "", true), &pc_facts(), 1_300), vec![CEv::Dropped("old-rseq")]);
    }

    #[test]
    fn pings_every_15_s_while_visible_only() {
        let mut c = opened(0);
        assert!(sent(&c.tick(PING_MS - 1)).is_empty());
        let p = sent(&c.tick(50 + PING_MS));
        assert_eq!(p.len(), 1);
        assert_eq!(p[0]["op"], "ping");
        assert_eq!(p[0]["vis"], true);
        let ps = p[0]["seq"].as_i64().unwrap();
        assert!(c.on_state(&state(SID, 2, ps, true, "", true), &pc_facts(), 15_100).contains(&CEv::Ack { seq: ps, ok: true, why: String::new(), op: "ping".into() }));
        let hide = sent(&c.set_visible(false, 20_000));
        assert_eq!(hide[0]["vis"], false, "at once on hiding");
        assert!(sent(&c.tick(20_000 + 3 * PING_MS)).iter().all(|m| m["op"] != "ping"));
        let show = sent(&c.set_visible(true, 80_000));
        assert_eq!(show[0]["vis"], true, "at once on becoming visible");
    }

    #[test]
    fn slider_pushes_are_paced_and_the_release_always_goes() {
        let mut c = opened(0);
        let live = |l: f64| json!({"op": "vol", "user": "@bob:lab.test", "cat": "mic", "level": l, "live": true});
        let mut out = 0;
        let mut t = 1_000;
        while t < 2_000 {
            out += c.send(&live(t as f64 / 2_000.0), t).1.len();
            out += sent(&c.tick(t)).len();
            t += 20;
        }
        assert!(out <= 4 + 1, "≤ 4/s, got {out}");
        let (seq, evs) = c.send(&json!({"op": "vol", "user": "@bob:lab.test", "cat": "mic", "level": 0.3}), 2_000);
        assert!(seq > 0 && sent(&evs).len() == 1, "the release goes at once");
        assert!(sent(&c.tick(2_500)).iter().all(|m| m["live"] != true), "no stale live push after the release");
        assert!(c.pending_ops().iter().any(|(_, op)| op == "vol"), "the release is acked");
        assert!(c.pending_ops().iter().all(|(s, _)| c.pending_content(*s).map(|m| m["live"] != true).unwrap_or(true)), "live pushes aren't");
    }

    #[test]
    fn sound_taps_are_paced_newest_wins() {
        let mut c = opened(0);
        let tap = |u: &str| json!({"op": "sound.play", "url": u});
        assert!(c.send(&tap("a"), 1_000).0 > 0);
        assert_eq!(c.send(&tap("b"), 1_050).0, 0);
        assert_eq!(c.send(&tap("c"), 1_100).0, 0);
        let evs = sent(&c.tick(1_250));
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0]["url"], "c");
    }

    #[test]
    fn unknown_sound_is_retried_once_after_2_s_as_a_new_command() {
        let mut c = opened(0);
        let (seq, _) = c.send(&json!({"op": "sound.play", "url": FAV}), 1_000);
        c.on_state(&state(SID, 2, seq, false, "unknown-sound", true), &pc_facts(), 1_100);
        assert!(sent(&c.tick(1_100 + SOUND_RETRY_MS - 1)).is_empty());
        let r = sent(&c.tick(1_100 + SOUND_RETRY_MS));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["url"], FAV);
        let seq2 = r[0]["seq"].as_i64().unwrap();
        assert!(seq2 > seq, "a new seq");
        let evs = c.on_state(&state(SID, 3, seq2, false, "unknown-sound", true), &pc_facts(), 3_200);
        assert!(evs.contains(&CEv::Ack { seq: seq2, ok: false, why: "unknown-sound".into(), op: "sound.play".into() }), "the second refusal shows");
        assert!(sent(&c.tick(10_000)).iter().all(|m| m["op"] != "sound.play"), "retried once only");
    }

    #[test]
    fn no_session_reopens_and_final_states_end() {
        let mut c = opened(0);
        let (seq, _) = c.send(&json!({"op": "brb", "on": true}), 1_000);
        let evs = c.on_state(&state(SID, 2, seq, false, "no-session", false), &pc_facts(), 1_100);
        assert!(evs.contains(&CEv::Reopen(SID.into())));
        let mut c = opened(0);
        let evs = c.on_state(&state(SID, 2, 0, false, "replaced", false), &pc_facts(), 1_100);
        assert!(evs.contains(&CEv::Ended("replaced".into())));
        assert_eq!(c.send(&json!({"op": "brb", "on": true}), 1_200).0, 0);
        // the receiver leaving its call is not the end of the session
        let mut c = opened(0);
        let evs = c.on_state(&state(SID, 2, 0, false, "ended", false), &pc_facts(), 1_100);
        assert!(!evs.iter().any(|e| matches!(e, CEv::Ended(_))));
        // a refused open ends it with its code
        let (mut c, _) = Controller::open(ROOM, PC, SID, 0);
        let evs = c.on_state(&state(SID, 1, 1, false, "off", false), &pc_facts(), 100);
        assert!(evs.contains(&CEv::Ended("off".into())));
    }

    #[test]
    fn stop_cancels_a_waiting_tap_and_a_call_change_drops_what_waited() {
        let mut c = opened(0);
        assert!(c.send(&json!({"op": "sound.play", "url": "a"}), 1_000).0 > 0);
        assert_eq!(c.send(&json!({"op": "sound.play", "url": "b"}), 1_050).0, 0);
        assert!(c.send(&json!({"op": "sound.stop"}), 1_100).0 > 0);
        assert!(sent(&c.tick(1_400)).iter().all(|m| m["op"] != "sound.play"), "the waiting tap is gone");
        // a waiting live slider push for the old call never reaches the new one
        let live = |l: f64| json!({"op": "vol", "user": "@bob:lab.test", "cat": "mic", "level": l, "live": true});
        c.send(&live(0.2), 2_000);
        assert_eq!(c.send(&live(0.3), 2_050).0, 0);
        let mut st = state(SID, 5, 0, true, "", true);
        st["call"]["gen"] = json!("g2");
        st["call"]["room"] = json!("!next:lab.test");
        c.on_state(&st, &pc_facts(), 2_100);
        assert_eq!((c.room.as_str(), c.gen.as_str()), ("!next:lab.test", "g2"), "the session follows the receiver's call");
        assert!(sent(&c.tick(2_400)).iter().all(|m| m["op"] != "vol"));
        let (_, evs) = c.send(&json!({"op": "brb", "on": true}), 2_500);
        assert_eq!(sent(&evs)[0]["room"], "!next:lab.test");
        assert_eq!(sent(&evs)[0]["gen"], "g2");
    }

    #[test]
    fn hidden_while_opening_tells_the_receiver_once_open() {
        let (mut c, _) = Controller::open(ROOM, PC, SID, 0);
        assert!(c.set_visible(false, 10).is_empty(), "nothing to tell before the answer");
        let evs = c.on_state(&state(SID, 1, 1, true, "", true), &pc_facts(), 100);
        let p = sent(&evs);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0]["op"], "ping");
        assert_eq!(p[0]["vis"], false);
    }

    #[test]
    fn an_ended_session_ignores_states() {
        let mut c = opened(0);
        c.on_state(&state(SID, 2, 0, false, "replaced", false), &pc_facts(), 100);
        assert_eq!(c.on_state(&state(SID, 3, 0, true, "", true), &pc_facts(), 200), vec![CEv::Dropped("ended")]);
    }

    #[test]
    fn states_from_another_device_or_session_are_dropped() {
        let mut c = opened(0);
        let other = PadFacts { sender_device: "LAPTOP".into(), ..pc_facts() };
        assert_eq!(c.on_state(&state(SID, 5, 0, true, "", true), &other, 100), vec![CEv::Dropped("other-device")]);
        assert_eq!(c.on_state(&state("Qm9vT2JhUmFjZTEz", 5, 0, true, "", true), &pc_facts(), 100), vec![CEv::Dropped("other-session")]);
        let untrusted = PadFacts { peer_trusted: false, ..pc_facts() };
        assert_eq!(c.on_state(&state(SID, 5, 0, true, "", true), &untrusted, 100), vec![CEv::Dropped("peer-untrusted")]);
    }

    #[test]
    fn the_room_list_bar_shows_the_newest_call() {
        // the same cases as the web's features/pad/order.test.ts
        let t = |ts: i64| json!([{"device": "PC", "createdTs": ts}]);
        let m = |pairs: Vec<(&str, Value)>| pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect::<Map<String, Value>>();
        assert_eq!(calls_newest_first(&m(vec![("!a:lab.test", t(1_000)), ("!b:lab.test", t(2_000))])), vec!["!b:lab.test", "!a:lab.test"]);
        assert_eq!(calls_newest_first(&m(vec![("!b:lab.test", t(5)), ("!a:lab.test", t(5))])), vec!["!a:lab.test", "!b:lab.test"], "ties: room id");
        assert_eq!(calls_newest_first(&m(vec![("!a:lab.test", json!([])), ("!b:lab.test", t(1))])), vec!["!b:lab.test"], "no target: left out");
        // the first target of a room is its newest (pad_targets' order): only it counts
        let two = json!([{"device": "PC", "createdTs": 9_000}, {"device": "LAPTOP", "createdTs": 1}]);
        assert_eq!(calls_newest_first(&m(vec![("!a:lab.test", t(8_000)), ("!c:lab.test", two)])), vec!["!c:lab.test", "!a:lab.test"]);
        assert!(calls_newest_first(&Map::new()).is_empty());
    }
}
