//! Call history from the rooms' call-membership state events — port of the web's `src/widgets/history.ts` (contract
//! WIDGETS.md §7, the precise v1 rules in §7.1; vectors `testvectors/widgets/history/*`). Pure: the app's cache feeds
//! it plain events (serde_json values, as the homeserver sent them).
//!
//! ```text
//! events → slots → device intervals → per-user unions (per room) → calls (per room) → the week per friend + last call
//! ```
//!
//! Every number is an integer count of milliseconds since the epoch held in a double (inputs are floored, like the
//! reference); every list has a fixed order, so both apps produce byte-identical canonical JSON for the same input.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value};

use crate::num;

/// The two membership event types read (the unstable MSC3401 name the apps write + the stable name).
pub const MEMBER_TYPES: [&str; 2] = ["org.matrix.msc3401.call.member", "m.call.member"];
/// `expires` when a membership has none (the apps' 4 h).
pub const DEFAULT_EXPIRES_MS: f64 = 14_400_000.0;
/// A membership never lasts longer than this after the event that states it (guards garbage `expires`/`created_ts`).
pub const EXPIRY_CAP_MS: f64 = 86_400_000.0;
pub const DAY_MS: f64 = 86_400_000.0;
/// "This week" = the last 7 days, [now − 7 d, now].
pub const WEEK_MS: f64 = 7.0 * DAY_MS;
/// How far back the apps fetch membership events (/messages with a type filter): the week + a day of margin.
pub const FETCH_MS: f64 = 8.0 * DAY_MS;
/// Inputs are bounded to ±2^53 so every sum and difference stays an exact integer.
pub const MAX_MS: f64 = 9_007_199_254_740_992.0;

/// One membership state event as the apps hand it over (timeline order within its room), already filtered to the
/// fields the rules read. Build with [`MemberEvent::from_json`] (the reference's shape: `{id?, type, key, sender, ts,
/// content}`) or [`MemberEvent::from_matrix`] (a raw Matrix event: `event_id`, `state_key`, `origin_server_ts`).
#[derive(Clone, Debug, PartialEq)]
pub struct MemberEvent {
    pub id: Option<String>,
    pub ty: String,
    pub key: String,
    pub sender: String,
    pub ts: f64,
    pub content: Value,
}

impl MemberEvent {
    /// The reference's `MemberEvent` shape; None when it fails roomOrder's type checks (not an object, `type` / `key` /
    /// `sender` not strings, `ts` not a finite number) — such events never take part.
    pub fn from_json(v: &Value) -> Option<MemberEvent> {
        let o = v.as_object()?;
        Some(MemberEvent {
            id: match o.get("id") {
                Some(Value::String(s)) => Some(s.clone()),
                _ => None,
            },
            ty: o.get("type")?.as_str()?.to_string(),
            key: o.get("key")?.as_str()?.to_string(),
            sender: o.get("sender")?.as_str()?.to_string(),
            ts: o.get("ts")?.as_f64()?,
            content: o.get("content").cloned().unwrap_or(Value::Null),
        })
    }

    /// A raw Matrix state event (sync / `/messages`) → the reference's shape.
    pub fn from_matrix(v: &Value) -> Option<MemberEvent> {
        let o = v.as_object()?;
        Some(MemberEvent {
            id: o.get("event_id").and_then(Value::as_str).map(str::to_string),
            ty: o.get("type")?.as_str()?.to_string(),
            key: o.get("state_key")?.as_str()?.to_string(),
            sender: o.get("sender")?.as_str()?.to_string(),
            ts: o.get("origin_server_ts")?.as_f64()?,
            content: o.get("content").cloned().unwrap_or(Value::Null),
        })
    }

    /// The reference's shape as JSON (the app's local cache stores this).
    pub fn to_json(&self) -> Value {
        let mut o = Map::new();
        if let Some(id) = &self.id {
            o.insert("id".into(), Value::String(id.clone()));
        }
        o.insert("type".into(), Value::String(self.ty.clone()));
        o.insert("key".into(), Value::String(self.key.clone()));
        o.insert("sender".into(), Value::String(self.sender.clone()));
        o.insert("ts".into(), num(self.ts));
        o.insert("content".into(), self.content.clone());
        Value::Object(o)
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct RoomEvents {
    pub room: String,
    pub events: Vec<MemberEvent>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct HistoryInput {
    /// my user id
    pub me: String,
    pub now: f64,
    pub rooms: Vec<RoomEvents>,
    /// the call recorder (left out of every call): `Some(Some(id))` = the account the server announces; `Some(None)`
    /// or `None` = this server names none (nobody is left out). The web's `recorder?: string | null`.
    pub recorder: Option<Option<String>>,
}

/// The `recorder` input as the web reads it: a string = that account ("" = nobody); absent or anything else (null) =
/// nobody.
pub fn recorder_input(v: &Value) -> Option<Option<String>> {
    match v.get("recorder") {
        None => None,
        Some(Value::String(s)) => Some(Some(s.clone())),
        Some(_) => Some(None),
    }
}

impl HistoryInput {
    /// The reference's input shape `{me, now, rooms: [{room, events: [...]}]}` (the vectors'). Rooms whose `room` is not
    /// a string or whose `events` is not an array are skipped, like computeHistory does.
    pub fn from_json(v: &Value) -> HistoryInput {
        let me = v.get("me").and_then(Value::as_str).unwrap_or("").to_string();
        let now = v.get("now").and_then(Value::as_f64).unwrap_or(f64::NAN);
        let mut rooms = Vec::new();
        for r in v.get("rooms").and_then(Value::as_array).into_iter().flatten() {
            // (a room entry that is not {room: string, events: array} is skipped — it never counts as "seen")
            if let (Some(room), Some(evs)) = (r.get("room").and_then(Value::as_str), r.get("events").and_then(Value::as_array)) {
                rooms.push(RoomEvents { room: room.to_string(), events: evs.iter().filter_map(MemberEvent::from_json).collect() });
            }
        }
        HistoryInput { me, now, rooms, recorder: recorder_input(v) }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct UserMs {
    pub user: String,
    pub ms: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub room: String,
    pub start: f64,
    pub end: f64,
    /// someone's membership is still live at `now` (then end = now)
    pub ongoing: bool,
    /// the member present at the call's first instant (the earliest opening event at `start`)
    pub started_by: String,
    /// everyone who was present, with their time in the call; user code-point order
    pub members: Vec<UserMs>,
    /// my time in the call (0 = I was not in it)
    pub my_ms: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Friend {
    pub user: String,
    /// time I spent in a call with them in the window
    pub together_ms: f64,
    /// calls in the window in which we overlapped
    pub calls: f64,
    /// of those: started by me / by someone else
    pub started: f64,
    pub joined: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Week {
    pub from: f64,
    pub to: f64,
    /// my time in calls in the window, and the calls I was in
    pub my_ms: f64,
    pub my_calls: f64,
    /// togetherMs descending, then user code-point order; only friends with calls > 0
    pub friends: Vec<Friend>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LastCall {
    pub room: String,
    /// the room call's bounds
    pub start: f64,
    pub end: f64,
    /// my first and last instant in it (the list is ranked by my_end — when I left)
    pub my_start: f64,
    pub my_end: f64,
    /// I am still in it at `now`
    pub ongoing: bool,
    pub started_by: String,
    pub my_ms: f64,
    /// who overlapped with me in it and for how long; ms descending, then user
    pub with: Vec<UserMs>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct History {
    /// every call in the data; start ascending, then room
    pub calls: Vec<Call>,
    pub week: Week,
    pub last: Option<LastCall>,
}

fn user_ms_json(u: &UserMs) -> Value {
    let mut o = Map::new();
    o.insert("user".into(), Value::String(u.user.clone()));
    o.insert("ms".into(), num(u.ms));
    Value::Object(o)
}

impl History {
    /// The reference's `History` object (key names as the vectors hold them).
    pub fn to_json(&self) -> Value {
        let calls: Vec<Value> = self
            .calls
            .iter()
            .map(|c| {
                let mut o = Map::new();
                o.insert("room".into(), Value::String(c.room.clone()));
                o.insert("start".into(), num(c.start));
                o.insert("end".into(), num(c.end));
                o.insert("ongoing".into(), Value::Bool(c.ongoing));
                o.insert("startedBy".into(), Value::String(c.started_by.clone()));
                o.insert("members".into(), Value::Array(c.members.iter().map(user_ms_json).collect()));
                o.insert("myMs".into(), num(c.my_ms));
                Value::Object(o)
            })
            .collect();
        let friends: Vec<Value> = self
            .week
            .friends
            .iter()
            .map(|f| {
                let mut o = Map::new();
                o.insert("user".into(), Value::String(f.user.clone()));
                o.insert("togetherMs".into(), num(f.together_ms));
                o.insert("calls".into(), num(f.calls));
                o.insert("started".into(), num(f.started));
                o.insert("joined".into(), num(f.joined));
                Value::Object(o)
            })
            .collect();
        let mut week = Map::new();
        week.insert("from".into(), num(self.week.from));
        week.insert("to".into(), num(self.week.to));
        week.insert("myMs".into(), num(self.week.my_ms));
        week.insert("myCalls".into(), num(self.week.my_calls));
        week.insert("friends".into(), Value::Array(friends));
        let last = match &self.last {
            None => Value::Null,
            Some(l) => {
                let mut o = Map::new();
                o.insert("room".into(), Value::String(l.room.clone()));
                o.insert("start".into(), num(l.start));
                o.insert("end".into(), num(l.end));
                o.insert("myStart".into(), num(l.my_start));
                o.insert("myEnd".into(), num(l.my_end));
                o.insert("ongoing".into(), Value::Bool(l.ongoing));
                o.insert("startedBy".into(), Value::String(l.started_by.clone()));
                o.insert("myMs".into(), num(l.my_ms));
                o.insert("with".into(), Value::Array(l.with.iter().map(user_ms_json).collect()));
                Value::Object(o)
            }
        };
        let mut o = Map::new();
        o.insert("calls".into(), Value::Array(calls));
        o.insert("week".into(), Value::Object(week));
        o.insert("last".into(), last);
        Value::Object(o)
    }
}

// ---- small pure helpers ---------------------------------------------------------------------------------------------

/// The call recorder: exactly the account the server announces (`Some(Some(id))`); with none announced (`None`,
/// `Some(None)` or an empty id) nobody is. The web's `isRecorderUser(user, recorder?)`.
pub fn is_recorder_user(user: &str, recorder: Option<Option<&str>>) -> bool {
    recorder.flatten().is_some_and(|r| !r.is_empty() && user == r)
}

/// A JSON number within ±2^53, floored; else None (absent).
fn int(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64).and_then(int_f)
}

fn int_f(x: f64) -> Option<f64> {
    (x.is_finite() && x.abs() <= MAX_MS).then(|| x.floor())
}

/// Is `key` a membership state key owned by (sender, device)? The apps' keyrules stateKeyOwned.
pub fn key_owned(key: &str, sender: &str, device: &str, array_form: bool) -> bool {
    if sender.is_empty() {
        return false;
    }
    if array_form {
        return key == sender;
    }
    if device.is_empty() {
        return false;
    }
    key == format!("_{sender}_{device}_m.call")
        || key == format!("_{sender}_{device}")
        || key == format!("{sender}_{device}")
        || key == format!("{sender}_{device}_m.call")
}

/// One device a membership event states: its device id and until when it lasts.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceState {
    pub dev: String,
    pub exp: f64,
}

/// The devices one membership event states (empty = the slot is empty: a leave, garbage, or a membership that is
/// already expired when it is sent). §7.1 rules 2–5.
pub fn devices_of(ev: &MemberEvent) -> Vec<DeviceState> {
    let ts = int_f(ev.ts).unwrap_or(0.0);
    let Some(c) = ev.content.as_object() else { return Vec::new() };
    if ev.sender.is_empty() {
        return Vec::new();
    }
    let mem = c.get("memberships");
    let array_form = matches!(mem, Some(Value::Array(_)));
    if !array_form && c.is_empty() {
        return Vec::new();
    }
    let single = [Value::Object(c.clone())];
    let entries: &[Value] = match mem {
        Some(Value::Array(a)) => a,
        _ => &single,
    };
    let mut out: Vec<DeviceState> = Vec::new();
    for e in entries {
        let Some(e) = e.as_object() else { continue };
        if let Some(app) = e.get("application") {
            if app.as_str() != Some("m.call") {
                continue;
            }
        }
        if let Some(scope) = e.get("scope") {
            if scope.as_str() != Some("m.room") {
                continue;
            }
        }
        let dev = match e.get("device_id") {
            Some(Value::String(s)) => s.as_str(),
            _ => "",
        };
        if dev.is_empty() || !key_owned(&ev.key, &ev.sender, dev, array_form) {
            continue;
        }
        if out.iter().any(|d| d.dev == dev) {
            continue;
        }
        let mut exp = match int(e.get("expires_ts")) {
            Some(abs) => abs,
            None => {
                let created = int(e.get("created_ts")).unwrap_or(ts);
                let expires = int(e.get("expires")).unwrap_or(DEFAULT_EXPIRES_MS);
                created + expires
            }
        };
        exp = crate::js_min(exp, ts + EXPIRY_CAP_MS);
        if exp <= ts {
            continue;
        }
        out.push(DeviceState { dev: dev.to_string(), exp });
    }
    out
}

// ---- intervals ------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct DeviceInterval {
    pub user: String,
    pub start: f64,
    pub end: f64,
    /// index (after dedupe + sort) of the event that opened it — the started-by tie-break
    pub open: usize,
    pub ongoing: bool,
}

/// The room's events to process, in the given (timeline) order — never re-sorted by ts: a known type, ts a number in
/// [0, 2^53] (floored) and ≤ now, a repeated non-empty id dropped (first kept). (The string checks happened in
/// [`MemberEvent::from_json`].)
pub fn room_order(events: &[MemberEvent], now: f64) -> Vec<MemberEvent> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut kept: Vec<MemberEvent> = Vec::new();
    for ev in events {
        if !MEMBER_TYPES.contains(&ev.ty.as_str()) {
            continue;
        }
        let Some(ts) = int_f(ev.ts) else { continue };
        if ts < 0.0 || ts > now {
            continue;
        }
        if let Some(id) = ev.id.as_deref().filter(|s| !s.is_empty()) {
            if !seen.insert(id) {
                continue;
            }
        }
        let mut e = ev.clone();
        e.ts = ts;
        kept.push(e);
    }
    kept
}

/// Every device's presence intervals in one room (recorders included; callers drop them). §7.1 rule 6.
pub fn device_intervals(events: &[MemberEvent], now: f64) -> Vec<DeviceInterval> {
    #[derive(Clone)]
    struct Rec {
        user: String,
        dev: String,
        since: f64,
        open: usize,
        exp: f64,
    }
    let order = room_order(events, now);
    // insertion-ordered map slot → recs (the reference's Map keeps insertion order; a deleted slot re-inserts at the end)
    let mut slot_order: Vec<String> = Vec::new();
    let mut slots: HashMap<String, Vec<Rec>> = HashMap::new();
    let mut out: Vec<DeviceInterval> = Vec::new();
    let close = |out: &mut Vec<DeviceInterval>, r: &Rec, end: f64, ongoing: bool| {
        if end > r.since {
            out.push(DeviceInterval { user: r.user.clone(), start: r.since, end, open: r.open, ongoing });
        }
    };
    for (i, ev) in order.iter().enumerate() {
        let slot = format!("{}\u{0}{}", ev.ty, ev.key);
        let prev = slots.get(&slot).cloned().unwrap_or_default();
        let devs = devices_of(ev);
        let mut next: Vec<Rec> = Vec::new();
        let mut continued: HashSet<String> = HashSet::new();
        for r in &prev {
            let same = devs.iter().find(|d| d.dev == r.dev && ev.sender == r.user);
            if r.exp <= ev.ts {
                close(&mut out, r, r.exp, false); // expired before this event
            } else if let Some(same) = same {
                let mut n = r.clone();
                n.exp = same.exp; // a refresh: the interval goes on, its expiry is the new one
                next.push(n);
                continued.insert(same.dev.clone());
            } else {
                close(&mut out, r, ev.ts, false);
            }
        }
        for d in &devs {
            if !continued.contains(&d.dev) {
                next.push(Rec { user: ev.sender.clone(), dev: d.dev.clone(), since: ev.ts, open: i, exp: d.exp });
            }
        }
        if !next.is_empty() {
            if !slots.contains_key(&slot) {
                slot_order.push(slot.clone());
            }
            slots.insert(slot, next);
        } else if slots.remove(&slot).is_some() {
            slot_order.retain(|s| s != &slot);
        }
    }
    for s in &slot_order {
        for r in &slots[s] {
            close(&mut out, r, crate::js_min(r.exp, now), r.exp > now);
        }
    }
    out.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.open.cmp(&b.open))
            .then(a.end.partial_cmp(&b.end).unwrap_or(std::cmp::Ordering::Equal))
    });
    out
}

// ---- interval sets --------------------------------------------------------------------------------------------------

pub type Span = (f64, f64);

/// Sorted, disjoint, merged (touching spans merge) union of [start, end) spans.
pub fn union(spans: &[Span]) -> Vec<Span> {
    let mut s: Vec<Span> = spans.iter().copied().filter(|(a, b)| b > a).collect();
    s.sort_by(|x, y| {
        x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal).then(x.1.partial_cmp(&y.1).unwrap_or(std::cmp::Ordering::Equal))
    });
    let mut out: Vec<Span> = Vec::new();
    for (a, b) in s {
        if let Some(last) = out.last_mut() {
            if a <= last.1 {
                last.1 = crate::js_max(last.1, b);
                continue;
            }
        }
        out.push((a, b));
    }
    out
}

/// Intersection of two unions.
pub fn intersect(x: &[Span], y: &[Span]) -> Vec<Span> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < x.len() && j < y.len() {
        let a = crate::js_max(x[i].0, y[j].0);
        let b = crate::js_min(x[i].1, y[j].1);
        if b > a {
            out.push((a, b));
        }
        if x[i].1 < y[j].1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// Total length of a union within [from, to].
pub fn length_in(u: &[Span], from: f64, to: f64) -> f64 {
    let mut n = 0.0;
    for &(a, b) in u {
        let s = crate::js_max(a, from);
        let e = crate::js_min(b, to);
        if e > s {
            n += e - s;
        }
    }
    n
}

// ---- calls ----------------------------------------------------------------------------------------------------------

struct RoomData {
    room: String,
    /// per user (recorders excluded): the union of their device intervals — insertion order kept
    users: Vec<(String, Vec<Span>)>,
    calls: Vec<Call>,
    /// one of my devices is still in this room's call at `now`
    my_ongoing: bool,
}

fn room_data(room: &str, events: &[MemberEvent], me: &str, now: f64, recorder: Option<Option<&str>>) -> RoomData {
    let ivs: Vec<DeviceInterval> = device_intervals(events, now).into_iter().filter(|iv| !is_recorder_user(&iv.user, recorder)).collect();
    let mut by_user: Vec<(String, Vec<Span>)> = Vec::new();
    for iv in &ivs {
        match by_user.iter_mut().find(|(u, _)| *u == iv.user) {
            Some((_, l)) => l.push((iv.start, iv.end)),
            None => by_user.push((iv.user.clone(), vec![(iv.start, iv.end)])),
        }
    }
    let users: Vec<(String, Vec<Span>)> = by_user.into_iter().map(|(u, l)| (u, union(&l))).collect();
    struct Cur {
        start: f64,
        end: f64,
        ongoing: bool,
        started_by: String,
    }
    let mut calls: Vec<Call> = Vec::new();
    let flush = |cur: &mut Option<Cur>, calls: &mut Vec<Call>| {
        let Some(c) = cur.take() else { return };
        let mut members: Vec<UserMs> = Vec::new();
        for (u, un) in &users {
            let ms = length_in(un, c.start, c.end);
            if ms > 0.0 {
                members.push(UserMs { user: u.clone(), ms });
            }
        }
        members.sort_by(|a, b| a.user.cmp(&b.user));
        let my_ms = members.iter().find(|m| m.user == me).map(|m| m.ms).unwrap_or(0.0);
        calls.push(Call { room: room.to_string(), start: c.start, end: c.end, ongoing: c.ongoing, started_by: c.started_by, members, my_ms });
    };
    let mut cur: Option<Cur> = None;
    for iv in &ivs {
        match cur.as_mut() {
            Some(c) if iv.start <= c.end => {
                if iv.end > c.end {
                    c.end = iv.end;
                }
                if iv.ongoing {
                    c.ongoing = true;
                }
            }
            _ => {
                flush(&mut cur, &mut calls);
                cur = Some(Cur { start: iv.start, end: iv.end, ongoing: iv.ongoing, started_by: iv.user.clone() });
            }
        }
    }
    flush(&mut cur, &mut calls);
    let my_ongoing = ivs.iter().any(|iv| iv.user == me && iv.ongoing);
    RoomData { room: room.to_string(), users, calls, my_ongoing }
}

/// The whole history: every call, the week per friend and my last call (§7.1 rules 7–10).
pub fn compute_history(input: &HistoryInput) -> History {
    let me = input.me.as_str();
    let now = int_f(input.now).unwrap_or(0.0);
    let mut rooms: Vec<RoomData> = Vec::new();
    let mut seen_rooms: HashSet<&str> = HashSet::new();
    for r in &input.rooms {
        if !seen_rooms.insert(r.room.as_str()) {
            continue;
        }
        rooms.push(room_data(&r.room, &r.events, me, now, input.recorder.as_ref().map(|r| r.as_deref())));
    }
    rooms.sort_by(|a, b| a.room.cmp(&b.room));
    let mut calls: Vec<Call> = rooms.iter().flat_map(|r| r.calls.iter().cloned()).collect();
    calls.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal).then(a.room.cmp(&b.room)));

    // ---- the week
    let from = now - WEEK_MS;
    let to = now;
    let mut friends: Vec<Friend> = Vec::new();
    fn friend<'a>(friends: &'a mut Vec<Friend>, u: &str) -> &'a mut Friend {
        if let Some(i) = friends.iter().position(|f| f.user == u) {
            return &mut friends[i];
        }
        friends.push(Friend { user: u.to_string(), together_ms: 0.0, calls: 0.0, started: 0.0, joined: 0.0 });
        friends.last_mut().unwrap()
    }
    let mut my_ms = 0.0;
    let mut my_calls = 0.0;
    let empty: Vec<Span> = Vec::new();
    for r in &rooms {
        let mine = r.users.iter().find(|(u, _)| u == me).map(|(_, s)| s).unwrap_or(&empty);
        my_ms += length_in(mine, from, to);
        // time together: my union ∩ theirs, within the window
        for (u, un) in &r.users {
            if u == me {
                continue;
            }
            let ov = length_in(&intersect(mine, un), from, to);
            if ov > 0.0 {
                friend(&mut friends, u).together_ms += ov;
            }
        }
        // calls in the window (the part inside it counts) that I was in; per friend who overlapped with me in it
        for c in &r.calls {
            if !(c.end > from && c.start < to) {
                continue;
            }
            let a = crate::js_max(c.start, from);
            let b = crate::js_min(c.end, to);
            if length_in(mine, a, b) <= 0.0 {
                continue;
            }
            my_calls += 1.0;
            for (u, un) in &r.users {
                if u == me {
                    continue;
                }
                if length_in(&intersect(mine, un), a, b) <= 0.0 {
                    continue;
                }
                let f = friend(&mut friends, u);
                f.calls += 1.0;
                if c.started_by == me {
                    f.started += 1.0;
                } else {
                    f.joined += 1.0;
                }
            }
        }
    }
    let mut list: Vec<Friend> = friends.into_iter().filter(|f| f.calls > 0.0).collect();
    list.sort_by(|a, b| {
        b.together_ms.partial_cmp(&a.together_ms).unwrap_or(std::cmp::Ordering::Equal).then(a.user.cmp(&b.user))
    });

    // ---- my last call: among calls I shared with someone, the one I LEFT last (my last instant in it), then my first
    // instant, then the smaller room id; its "when" is when I left, its length my time in it
    let mut last: Option<LastCall> = None;
    for r in &rooms {
        let mine = r.users.iter().find(|(u, _)| u == me).map(|(_, s)| s).unwrap_or(&empty);
        for c in &r.calls {
            if c.my_ms <= 0.0 {
                continue;
            }
            let mut with: Vec<UserMs> = Vec::new();
            for (u, un) in &r.users {
                if u == me {
                    continue;
                }
                let ms = length_in(&intersect(mine, un), c.start, c.end);
                if ms > 0.0 {
                    with.push(UserMs { user: u.clone(), ms });
                }
            }
            if with.is_empty() {
                continue;
            }
            with.sort_by(|a, b| b.ms.partial_cmp(&a.ms).unwrap_or(std::cmp::Ordering::Equal).then(a.user.cmp(&b.user)));
            let inside = intersect(mine, &[(c.start, c.end)]);
            let (Some(first), Some(lastspan)) = (inside.first(), inside.last()) else { continue };
            let (my_start, my_end) = (first.0, lastspan.1);
            let better = match &last {
                None => true,
                Some(l) => my_end > l.my_end || (my_end == l.my_end && (my_start > l.my_start || (my_start == l.my_start && c.room < l.room))),
            };
            if better {
                last = Some(LastCall {
                    room: c.room.clone(),
                    start: c.start,
                    end: c.end,
                    my_start,
                    my_end,
                    ongoing: r.my_ongoing && my_end == now,
                    started_by: c.started_by.clone(),
                    my_ms: c.my_ms,
                    with,
                });
            }
        }
    }
    History { calls, week: Week { from, to, my_ms, my_calls, friends: list }, last }
}

#[cfg(test)]
mod recorder_tests {
    use super::*;

    /// The `recorder` input (the web's `recorder?: string | null`): a string = exactly that account; absent or null =
    /// nobody (there is no default recorder account).
    #[test]
    fn the_recorder_input_absent_an_account_or_none() {
        assert!(!is_recorder_user("@recorder:example.com", None));
        assert!(is_recorder_user("@recorder:example.com", Some(Some("@recorder:example.com"))));
        assert!(!is_recorder_user("@vod:example.com", Some(Some("@recorder:example.com"))));
        assert!(!is_recorder_user("@recorder:example.com", Some(None)));
        assert!(!is_recorder_user("", Some(Some(""))));
        assert_eq!(recorder_input(&serde_json::json!({})), None);
        assert_eq!(recorder_input(&serde_json::json!({ "recorder": null })), Some(None));
        assert_eq!(recorder_input(&serde_json::json!({ "recorder": "@r:x" })), Some(Some("@r:x".to_string())));
        // the vector with the recorder in the call (its input names `@recorder:example.com` as the recorder): left out
        // as announced; with another recorder announced, null, or none at all it is a person like any other
        let v: serde_json::Value = serde_json::from_str(include_str!("../testvectors/widgets/history/hand-recorder-excluded.json")).unwrap();
        let with = |r: Option<serde_json::Value>| {
            let mut i = v["input"].clone();
            match r {
                Some(r) => i["recorder"] = r,
                None => {
                    i.as_object_mut().unwrap().remove("recorder");
                }
            }
            let h = compute_history(&HistoryInput::from_json(&i));
            h.calls.iter().flat_map(|c| c.members.iter().map(|m| m.user.clone())).collect::<Vec<_>>()
        };
        assert_eq!(with(Some(serde_json::json!("@recorder:example.com"))), ["@ana:example.com", "@me:example.com"]);
        assert!(with(Some(serde_json::json!("@other:example.com"))).contains(&"@recorder:example.com".to_string()));
        assert!(with(Some(serde_json::Value::Null)).contains(&"@recorder:example.com".to_string()));
        assert!(with(None).contains(&"@recorder:example.com".to_string()), "absent = no recorder");
    }
}
