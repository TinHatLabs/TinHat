//! guests.rs — the guest room's pure rules (contract: docs/GUESTS.md in the server repo; the desktop half:
//! docs/GUESTS.md here). SHARED LOGIC with Tin Hat Web's `src/features/guests/model.ts`, line for line; the tests below
//! replay `model.test.ts`'s cases (the words byte for byte). Pure: no Qt, no network, no store — the controller is
//! guests_ctl.rs.
//!
//!   parse_guest_state / parse_waiting   the room state the server keeps (`dev.tinhat.guests`, `dev.tinhat.guests.waiting`)
//!   parse_doc                           GET /api/guests → the doc (link, roles, room)
//!   local_role / merge_role             who may do what (the server's own rule, from the room's power levels + the doc)
//!   bar_plan / waiting_rows             the bar under the Guests room's header and its waiting rows
//!   pane_plan                           Settings → Guests
//!   is_guests_room                      is this room THE Guests room (the doc's room; else the alias the server gave it)
//!   arrivals / departures               the "is waiting" notices: new ids only, once per device (dedupe by id)
//!   account_name                        the account footer's name (a guest's display name, everyone else's localpart)
//!
//! The web's `handoffJoinsCall` is web only (the hand-off is a browser page): nothing for the desktop.

use std::collections::HashSet;

use serde_json::{Map, Value};

pub use crate::server_offer::is_guest_account;

pub const GUESTS_TYPE: &str = "dev.tinhat.guests";
pub const WAITING_TYPE: &str = "dev.tinhat.guests.waiting";
/// the server keeps at most this many waiting
pub const MAX_WAITING: usize = 20;
/// a name is at most this many characters (code points), as the server cuts it
pub const NAME_MAX: usize = 32;
/// a knock waits this long (ms); an older entry is never announced
pub const WAIT_MS: f64 = 10.0 * 60_000.0;
/// turn on / off and new link: a server admin or this power level in the Guests room
pub const MANAGE_LEVEL: f64 = 50.0;
/// how many announced ids a device remembers
pub const SEEN_MAX: usize = 100;

// ---- words (docs/GUESTS.md "Words"; the web uses exactly these) ------------------------------------------------

/// model.ts `WORDS`, byte for byte.
pub struct Words {
    pub pane: &'static str,
    pub turn_on_long: &'static str,
    pub turn_on: &'static str,
    pub turn_off: &'static str,
    pub copy: &'static str,
    pub new_link: &'static str,
    pub let_in: &'static str,
    pub turn_away: &'static str,
    pub open: &'static str,
    pub bar_on: &'static str,
    pub bar_off_manager: &'static str,
    pub bar_off: &'static str,
    pub pane_on: &'static str,
    pub pane_off: &'static str,
    pub pane_never: &'static str,
    pub pane_never_member: &'static str,
    pub unavailable: &'static str,
    pub someone: &'static str,
}

pub const WORDS: Words = Words {
    pane: "Guests",
    turn_on_long: "Turn on the guest room",
    turn_on: "Turn on",
    turn_off: "Turn off",
    copy: "Copy guest link",
    new_link: "New link",
    let_in: "Let in",
    turn_away: "Turn away",
    open: "Open the Guests room",
    bar_on: "Guest room is on",
    bar_off_manager: "Guest room is off",
    bar_off: "The guest room is off",
    pane_on: "The guest room is on.",
    pane_off: "The guest room is off.",
    pane_never: "The guest room hasn't been turned on yet.",
    pane_never_member: "The guest room hasn't been turned on yet. A server admin can turn it on here.",
    unavailable: "The guest room isn't available on this server.",
    someone: "A guest",
};

/// Settings → Guests: what the guest room is (two sentences).
pub const EXPLAIN: &str = "The guest room lets people without an account join you for a while: they open the guest link in a browser, type a name and wait at the door until someone here lets them in. Guests stay in the Guests room (and its call) for at most 12 hours, then their temporary account disappears.";

pub fn waiting_text(name: &str) -> String {
    format!("{name} is waiting")
}

pub fn notice_text(name: &str) -> String {
    format!("{name} is waiting to come in as a guest")
}

// ---- JS's value rules -------------------------------------------------------------------------------------------

/// JS `\s` and `String.prototype.trim`: WhiteSpace (TAB, VT, FF, SP, NBSP, ZWNBSP, the Zs spaces) + LineTerminator
/// (LF, CR, LS, PS) — not Rust's `char::is_whitespace` (which also takes U+0085).
fn js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\u{0B}'
            | '\u{0C}'
            | '\r'
            | ' '
            | '\u{A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'
    )
}

fn js_trim(s: &str) -> &str {
    s.trim_matches(js_space)
}

/// model.ts `num`: a finite number ≥ 0, else none.
fn num(v: Option<&Value>) -> Option<f64> {
    match v {
        Some(Value::Number(n)) => n.as_f64().filter(|x| x.is_finite() && *x >= 0.0),
        _ => None,
    }
}

/// model.ts `intOr`: an integer (`Number.isInteger`), else `d`.
fn int_or(v: Option<&Value>, d: f64) -> f64 {
    match v {
        Some(Value::Number(n)) => match n.as_f64() {
            Some(x) if x.is_finite() && x.fract() == 0.0 => x,
            _ => d,
        },
        _ => d,
    }
}

// ---- the room state ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GuestRoomState {
    pub on: bool,
    /// when it was last turned on / off (ms); None when not said
    pub since: Option<f64>,
}

/// `dev.tinhat.guests` → on/off; None when there is no usable event (no content, or `on` isn't a boolean).
pub fn parse_guest_state(content: &Value) -> Option<GuestRoomState> {
    let o = content.as_object()?;
    let on = match o.get("on") {
        Some(Value::Bool(b)) => *b,
        _ => return None,
    };
    Some(GuestRoomState { on, since: num(o.get("since")) })
}

#[derive(Clone, Debug, PartialEq)]
pub struct Waiter {
    /// 8 characters [A-Za-z0-9_-] (the server's id; the admit / turn-away key)
    pub id: String,
    pub name: String,
    /// when they knocked (ms); 0 when not said
    pub at: f64,
}

/// A waiting guest's name as shown: trimmed, at most NAME_MAX code points, "A guest" when empty.
pub fn clean_name(v: &Value) -> String {
    let s = match v {
        Value::String(s) => {
            let cut: String = js_trim(s).chars().take(NAME_MAX).collect();
            js_trim(&cut).to_string()
        }
        _ => String::new(),
    };
    if s.is_empty() {
        WORDS.someone.to_string()
    } else {
        s
    }
}

/// model.ts `ID_RE` = /^[A-Za-z0-9_-]{8}$/ (a waiting guest's id; the admit / turn-away key)
pub fn is_waiter_id(s: &str) -> bool {
    s.len() == 8 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `dev.tinhat.guests.waiting` (or the doc's `waiting` array) → who waits, in the server's order: valid ids only, the
/// first of a repeated id, at most MAX_WAITING. Anything malformed → [].
pub fn parse_waiting(content: &Value) -> Vec<Waiter> {
    let list = match content {
        Value::Array(a) => a,
        Value::Object(o) => match o.get("waiting") {
            Some(Value::Array(a)) => a,
            _ => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for w in list {
        if out.len() >= MAX_WAITING {
            break;
        }
        let Some(w) = w.as_object() else { continue };
        let id = match w.get("id") {
            Some(Value::String(s)) if is_waiter_id(s) && !seen.contains(s.as_str()) => s,
            _ => continue,
        };
        seen.insert(id.as_str());
        out.push(Waiter { id: id.clone(), name: clean_name(w.get("name").unwrap_or(&Value::Null)), at: num(w.get("at")).unwrap_or(0.0) });
    }
    out
}

// ---- the doc (GET /api/guests) ----------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct GuestsDoc {
    /// the server can run the guest room at all
    pub available: bool,
    /// the Guests room; None before the first "Turn on"
    pub room: Option<String>,
    pub alias: String,
    pub on: bool,
    pub since: f64,
    /// the guest link — only for members who aren't guests, and managers
    pub link: Option<String>,
    pub waiting: Vec<Waiter>,
    pub can_manage: bool,
    pub can_admit: bool,
    pub member: bool,
}

/// The doc's side of the roles (model.ts `Pick<GuestsDoc, "member" | "canAdmit" | "canManage">`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct DocRole {
    pub member: bool,
    pub can_admit: bool,
    pub can_manage: bool,
}

impl GuestsDoc {
    pub fn role(&self) -> DocRole {
        DocRole { member: self.member, can_admit: self.can_admit, can_manage: self.can_manage }
    }
}

/// /^![^:\s]+:\S+$/
fn is_room_id(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('!') else { return false };
    let Some((local, server)) = rest.split_once(':') else { return false };
    !local.is_empty() && !local.contains(js_space) && !server.is_empty() && !server.contains(js_space)
}

/// /^https?:\/\/\S+$/
fn is_link(s: &str) -> bool {
    let rest = s.strip_prefix("https://").or_else(|| s.strip_prefix("http://"));
    rest.is_some_and(|r| !r.is_empty() && !r.contains(js_space))
}

/// The API's answer → a doc; None when it isn't an object. Unknown / odd fields read as "no".
pub fn parse_doc(v: &Value) -> Option<GuestsDoc> {
    let o: &Map<String, Value> = v.as_object()?;
    let yes = |k: &str| o.get(k) == Some(&Value::Bool(true));
    let room = match o.get("room") {
        Some(Value::String(s)) if is_room_id(s) => Some(s.clone()),
        _ => None,
    };
    let alias = match o.get("alias") {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    let link = match o.get("link") {
        Some(Value::String(s)) if is_link(s) => Some(s.clone()),
        _ => None,
    };
    Some(GuestsDoc {
        available: yes("available"),
        room,
        alias,
        on: yes("on"),
        since: num(o.get("since")).unwrap_or(0.0),
        link,
        waiting: parse_waiting(o.get("waiting").unwrap_or(&Value::Null)),
        can_manage: yes("canManage"),
        can_admit: yes("canAdmit"),
        member: yes("member"),
    })
}

// ---- who may do what --------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct GuestRole {
    /// joined to the Guests room
    pub member: bool,
    /// a temporary guest account
    pub guest: bool,
    /// see the state, copy the link, let in / turn away
    pub can_admit: bool,
    /// turn on / off, new link
    pub can_manage: bool,
}

/// The role this device can tell by itself (the server's own rule, docs/GUESTS.md "Who may do what"): admit = joined,
/// not a guest, power level ≥ the room's `invite` level (0 when unset); manage = joined, not a guest, power level ≥ 50.
/// A server admin who isn't a member only shows through the doc (merge_role).
pub fn local_role(user_id: &str, membership: Option<&str>, power_levels: &Value) -> GuestRole {
    let guest = is_guest_account(user_id);
    let member = membership == Some("join");
    let empty = Map::new();
    let pl = power_levels.as_object().unwrap_or(&empty);
    let users = pl.get("users").and_then(Value::as_object).unwrap_or(&empty);
    let mine = int_or(users.get(user_id), int_or(pl.get("users_default"), 0.0));
    let invite = int_or(pl.get("invite"), 0.0);
    GuestRole { member, guest, can_admit: !guest && member && mine >= invite, can_manage: !guest && member && mine >= MANAGE_LEVEL }
}

/// This device's view + the server's answer (either may say yes: sync is fresher about membership, the doc knows
/// server admins). A guest account never gets a right, whatever the doc says.
pub fn merge_role(local: GuestRole, doc: Option<DocRole>) -> GuestRole {
    let guest = local.guest;
    let d = doc.unwrap_or_default();
    GuestRole {
        member: local.member || d.member,
        guest,
        can_admit: !guest && (local.can_admit || d.can_admit),
        can_manage: !guest && (local.can_manage || d.can_manage),
    }
}

/// May see the state and the link (docs/GUESTS.md: any member who isn't a guest; and managers).
pub fn sees_guest_room(r: GuestRole) -> bool {
    !r.guest && (r.can_admit || r.can_manage)
}

// ---- the bar under the Guests room's header ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarButton {
    Copy,
    NewLink,
    TurnOff,
    TurnOn,
}

impl BarButton {
    /// the web's key (`copy`, `newLink`, `turnOff`, `turnOn`)
    pub fn id(self) -> &'static str {
        match self {
            BarButton::Copy => "copy",
            BarButton::NewLink => "newLink",
            BarButton::TurnOff => "turnOff",
            BarButton::TurnOn => "turnOn",
        }
    }
    /// model.ts `BAR_LABEL`
    pub fn label(self) -> &'static str {
        match self {
            BarButton::Copy => WORDS.copy,
            BarButton::NewLink => WORDS.new_link,
            BarButton::TurnOff => WORDS.turn_off,
            BarButton::TurnOn => WORDS.turn_on,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarPlan {
    pub text: &'static str,
    pub buttons: Vec<BarButton>,
}

/// The bar (docs/GUESTS.md "The apps" 2): on — "Guest room is on · [Copy guest link] [New link] [Turn off]" (New link
/// / Turn off for managers only); off — "Guest room is off · [Turn on]" (managers) or "The guest room is off"
/// (others). Guests (and anyone who may not see the state) get no bar at all.
pub fn bar_plan(on: bool, role: GuestRole) -> Option<BarPlan> {
    if !sees_guest_room(role) {
        return None;
    }
    if on {
        let buttons = if role.can_manage { vec![BarButton::Copy, BarButton::NewLink, BarButton::TurnOff] } else { vec![BarButton::Copy] };
        return Some(BarPlan { text: WORDS.bar_on, buttons });
    }
    Some(if role.can_manage {
        BarPlan { text: WORDS.bar_off_manager, buttons: vec![BarButton::TurnOn] }
    } else {
        BarPlan { text: WORDS.bar_off, buttons: Vec::new() }
    })
}

/// The rows under the bar: everyone waiting, while the room is on, for those who may let them in.
pub fn waiting_rows(on: bool, role: GuestRole, waiting: &[Waiter]) -> Vec<Waiter> {
    if on && !role.guest && role.can_admit {
        waiting.to_vec()
    } else {
        Vec::new()
    }
}

// ---- Settings → Guests ------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneButton {
    TurnOn,
    TurnOff,
    Copy,
    NewLink,
    Open,
}

impl PaneButton {
    pub fn id(self) -> &'static str {
        match self {
            PaneButton::TurnOn => "turnOn",
            PaneButton::TurnOff => "turnOff",
            PaneButton::Copy => "copy",
            PaneButton::NewLink => "newLink",
            PaneButton::Open => "open",
        }
    }
    /// model.ts `PANE_LABEL`
    pub fn label(self) -> &'static str {
        match self {
            PaneButton::TurnOn => WORDS.turn_on_long,
            PaneButton::TurnOff => WORDS.turn_off,
            PaneButton::Copy => WORDS.copy,
            PaneButton::NewLink => WORDS.new_link,
            PaneButton::Open => WORDS.open,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneState {
    Unavailable,
    Never,
    On,
    Off,
}

impl PaneState {
    pub fn name(self) -> &'static str {
        match self {
            PaneState::Unavailable => "unavailable",
            PaneState::Never => "never",
            PaneState::On => "on",
            PaneState::Off => "off",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanePlan {
    pub state: PaneState,
    pub text: &'static str,
    pub buttons: Vec<PaneButton>,
    /// show the guest link as text (to copy by hand)
    pub show_link: bool,
}

/// Settings → Guests (docs/GUESTS.md "The apps" 1): the state, and — managers: Turn on the guest room / Turn off · Copy
/// guest link · New link · Open the Guests room; members: Copy guest link (while on) · Open the Guests room; everyone
/// else who isn't a guest: Open the Guests room once it exists (its join rule is public — opening it joins it).
pub fn pane_plan(doc: &GuestsDoc, role: GuestRole) -> PanePlan {
    use PaneButton::*;
    if !doc.available {
        return PanePlan { state: PaneState::Unavailable, text: WORDS.unavailable, buttons: Vec::new(), show_link: false };
    }
    if role.guest {
        let state = match (&doc.room, doc.on) {
            (None, _) => PaneState::Never,
            (Some(_), true) => PaneState::On,
            (Some(_), false) => PaneState::Off,
        };
        return PanePlan { state, text: if doc.on { WORDS.pane_on } else { WORDS.pane_off }, buttons: Vec::new(), show_link: false };
    }
    if doc.room.is_none() {
        return PanePlan {
            state: PaneState::Never,
            text: if role.can_manage { WORDS.pane_never } else { WORDS.pane_never_member },
            buttons: if role.can_manage { vec![TurnOn] } else { Vec::new() },
            show_link: false,
        };
    }
    let sees = sees_guest_room(role);
    if doc.on {
        let buttons = if role.can_manage {
            vec![Copy, NewLink, TurnOff, Open]
        } else if sees {
            vec![Copy, Open]
        } else {
            vec![Open]
        };
        return PanePlan { state: PaneState::On, text: WORDS.pane_on, buttons, show_link: sees && doc.link.is_some() };
    }
    PanePlan { state: PaneState::Off, text: WORDS.pane_off, buttons: if role.can_manage { vec![TurnOn, Open] } else { vec![Open] }, show_link: false }
}

// ---- which room is the Guests room ------------------------------------------------------------------------------

/// THE Guests room: the doc's `room` once the server answered; before that, a room carrying `dev.tinhat.guests` whose
/// canonical alias is `#guests:<server>` (only the server's system account can set it there — power level 100). A room
/// where someone else wrote the state event is never one.
pub fn is_guests_room(room_id: &str, doc_room: Option<&str>, has_guest_state: bool, canonical_alias: Option<&str>, server: &str) -> bool {
    if let Some(d) = doc_room.filter(|d| !d.is_empty()) {
        return room_id == d;
    }
    has_guest_state && !server.is_empty() && canonical_alias == Some(format!("#guests:{server}").as_str())
}

// ---- noticing (the "is waiting" notification) -------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Arrivals {
    /// who to announce now
    pub notify: Vec<Waiter>,
    /// the ids this device has announced (oldest first, the newest SEEN_MAX)
    pub seen: Vec<String>,
}

/// Who to announce now: each id once per device (dedupe by id — a restart, a re-sync or the state event arriving again
/// never repeats it), only while eligible (an id seen while not eligible is announced once eligible, if it still
/// waits), never an entry older than WAIT_MS (its knock is over). `seen` keeps the newest SEEN_MAX ids. `eligible` =
/// this user may let people in: a member of the Guests room who isn't a guest, and the server offers the guest room.
pub fn arrivals(seen: &[String], waiting: &[Waiter], now: f64, eligible: bool) -> Arrivals {
    if !eligible {
        return Arrivals { notify: Vec::new(), seen: seen.to_vec() };
    }
    let mut known: HashSet<&str> = seen.iter().map(String::as_str).collect();
    let mut notify = Vec::new();
    let mut out: Vec<String> = seen.to_vec();
    for w in waiting {
        if known.contains(w.id.as_str()) {
            continue;
        }
        known.insert(w.id.as_str());
        out.push(w.id.clone());
        let stale = w.at > 0.0 && now - w.at >= WAIT_MS;
        if !stale {
            notify.push(w.clone());
        }
    }
    let cut = out.len().saturating_sub(SEEN_MAX);
    Arrivals { notify, seen: out.split_off(cut) }
}

/// The ids that left the list (let in or turned away — by anyone): their notice closes.
pub fn departures(before: &[String], waiting: &[Waiter]) -> Vec<String> {
    let now: HashSet<&str> = waiting.iter().map(|w| w.id.as_str()).collect();
    before.iter().filter(|id| !now.contains(id.as_str())).cloned().collect()
}

// ---- a guest's own name -----------------------------------------------------------------------------------------

/// The name the account footer shows: everyone's localpart (as the desktop always did), except a temporary guest
/// account's — its localpart is a random id ("guest-9b230bc…"), so the display name the server registered ("Cousin Jo
/// (guest)") is shown, the localpart only when there is none. (The desktop signs in with a password: no guest account
/// in practice, so its footer keeps the localpart; the rule is here for the shared tests.)
#[cfg_attr(not(test), allow(dead_code))]
pub fn account_name(user_id: &str, display_name: Option<&str>) -> String {
    let local = user_id.strip_prefix('@').unwrap_or(user_id).split(':').next().unwrap_or("").to_string();
    let name = display_name.map(js_trim).unwrap_or("");
    if is_guest_account(user_id) && !name.is_empty() {
        name.to_string()
    } else {
        local
    }
}

#[cfg(test)]
mod tests {
    //! model.test.ts's cases (Tin Hat Web, branch selfhost), one Rust test per web test. The web's last two
    //! ("a guest account gets no Report a problem" — server_offer.rs's tests; "handoffJoinsCall" — web only) are not
    //! here.
    use super::*;
    use serde_json::json;

    const ME: &str = "@ana:maple.example";
    const ROOM: &str = "!guests:maple.example";

    fn role(member: bool, guest: bool, can_admit: bool, can_manage: bool) -> GuestRole {
        GuestRole { member, guest, can_admit, can_manage }
    }
    const MEMBER: GuestRole = GuestRole { member: true, guest: false, can_admit: true, can_manage: false };
    const MANAGER: GuestRole = GuestRole { member: true, guest: false, can_admit: true, can_manage: true };
    const GUEST: GuestRole = GuestRole { member: true, guest: true, can_admit: false, can_manage: false };
    const OUTSIDER: GuestRole = GuestRole { member: false, guest: false, can_admit: false, can_manage: false };
    const NONE: Vec<Waiter> = Vec::new();

    fn w(id: &str, name: &str, at: f64) -> Waiter {
        Waiter { id: id.into(), name: name.into(), at }
    }
    fn doc() -> GuestsDoc {
        GuestsDoc {
            available: true,
            room: Some(ROOM.into()),
            alias: "#guests:maple.example".into(),
            on: true,
            since: 1.0,
            link: Some("https://maple.example/call/?g=abcdefghijklmnopqrstuvwx".into()),
            waiting: Vec::new(),
            can_manage: false,
            can_admit: true,
            member: true,
        }
    }
    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_words_and_the_event_types() {
        assert_eq!(GUESTS_TYPE, "dev.tinhat.guests");
        assert_eq!(WAITING_TYPE, "dev.tinhat.guests.waiting");
        let bar: Vec<(&str, &str)> = [BarButton::Copy, BarButton::NewLink, BarButton::TurnOff, BarButton::TurnOn].iter().map(|b| (b.id(), b.label())).collect();
        assert_eq!(bar, [("copy", "Copy guest link"), ("newLink", "New link"), ("turnOff", "Turn off"), ("turnOn", "Turn on")]);
        let pane: Vec<(&str, &str)> = [PaneButton::TurnOn, PaneButton::TurnOff, PaneButton::Copy, PaneButton::NewLink, PaneButton::Open].iter().map(|b| (b.id(), b.label())).collect();
        assert_eq!(
            pane,
            [("turnOn", "Turn on the guest room"), ("turnOff", "Turn off"), ("copy", "Copy guest link"), ("newLink", "New link"), ("open", "Open the Guests room")]
        );
        assert_eq!(WORDS.let_in, "Let in");
        assert_eq!(WORDS.turn_away, "Turn away");
        assert_eq!(waiting_text("Ana"), "Ana is waiting");
        assert_eq!(notice_text("Ana"), "Ana is waiting to come in as a guest");
        assert_eq!(EXPLAIN.split(". ").count(), 2, "two sentences");
        // the rest of WORDS, byte for byte (model.ts; the web test pins them through barPlan / panePlan below)
        assert_eq!(WORDS.pane, "Guests");
        assert_eq!(WORDS.someone, "A guest");
        assert_eq!(
            EXPLAIN,
            "The guest room lets people without an account join you for a while: they open the guest link in a browser, type a name and wait at the door until someone here lets them in. Guests stay in the Guests room (and its call) for at most 12 hours, then their temporary account disappears."
        );
        assert_eq!((MAX_WAITING, NAME_MAX, WAIT_MS, MANAGE_LEVEL, SEEN_MAX), (20, 32, 600_000.0, 50.0, 100));
    }

    #[test]
    fn parse_guest_state_on_off_anything_without_a_boolean_on_is_no_state() {
        assert_eq!(parse_guest_state(&json!({ "v": 1, "on": true, "since": 1790000000000u64 })), Some(GuestRoomState { on: true, since: Some(1790000000000.0) }));
        assert_eq!(parse_guest_state(&json!({ "v": 1, "on": false, "since": 5 })), Some(GuestRoomState { on: false, since: Some(5.0) }));
        assert_eq!(parse_guest_state(&json!({ "on": true })), Some(GuestRoomState { on: true, since: None }));
        assert_eq!(parse_guest_state(&json!({ "on": true, "since": -1 })), Some(GuestRoomState { on: true, since: None }));
        assert_eq!(parse_guest_state(&json!({ "on": true, "since": "5" })), Some(GuestRoomState { on: true, since: None }));
        assert_eq!(parse_guest_state(&json!({ "v": 2, "on": true, "since": 3 })), Some(GuestRoomState { on: true, since: Some(3.0) }), "a newer v still says on/off");
        for v in [Value::Null, json!({}), json!([]), json!("on"), json!({ "on": "true" }), json!({ "on": 1 }), json!({ "v": 1 })] {
            assert_eq!(parse_guest_state(&v), None, "{v}");
        }
    }

    #[test]
    fn parse_waiting_valid_ids_first_of_a_repeat_at_most_20_names_cleaned() {
        assert_eq!(parse_waiting(&json!({ "v": 1, "waiting": [{ "id": "abcd1234", "name": "Ana", "at": 100 }] })), vec![w("abcd1234", "Ana", 100.0)]);
        assert_eq!(parse_waiting(&json!({ "v": 1, "waiting": [] })), NONE);
        for v in [Value::Null, json!({}), json!({ "waiting": "x" }), json!({ "waiting": {} }), json!(7)] {
            assert_eq!(parse_waiting(&v), NONE, "{v}");
        }
        let got = parse_waiting(&json!({
            "waiting": [
                { "id": "abcd1234", "name": "  Ana  ", "at": 1 },
                { "id": "abcd1234", "name": "Ana again", "at": 2 },
                { "id": "short", "name": "Bo" },
                { "id": "abcd12345", "name": "Cy" },
                { "id": "ab d1234", "name": "Di" },
                { "id": 12345678, "name": "Ed" },
                null,
                "x",
                { "id": "Zz_-0912", "name": "", "at": "soon" },
                { "id": "Yy_-0912", "name": 7, "at": -5 },
            ]
        }));
        assert_eq!(got, vec![w("abcd1234", "Ana", 1.0), w("Zz_-0912", "A guest", 0.0), w("Yy_-0912", "A guest", 0.0)]);
        let many: Vec<Value> = (0..30).map(|i| json!({ "id": format!("id{i:06}"), "name": format!("G{i}"), "at": i })).collect();
        let cut = parse_waiting(&json!({ "waiting": many }));
        assert_eq!(cut.len(), MAX_WAITING);
        assert_eq!(cut[0].id, "id000000");
        assert_eq!(cut[19].id, "id000019");
        assert_eq!(parse_waiting(&json!([{ "id": "abcd1234", "name": "Ana", "at": 1 }])), vec![w("abcd1234", "Ana", 1.0)], "the doc's bare array");
    }

    #[test]
    fn clean_name_trimmed_at_most_32_code_points_never_empty() {
        assert_eq!(clean_name(&json!(" Ana ")), "Ana");
        assert_eq!(clean_name(&json!("x".repeat(40))), "x".repeat(32));
        assert_eq!(clean_name(&json!("🙂".repeat(40))), "🙂".repeat(32), "code points, not UTF-16 units");
        assert_eq!(clean_name(&json!(format!("{} b", "a".repeat(31)))), "a".repeat(31), "cut, then trimmed again");
        for v in [json!(""), json!("   "), Value::Null, json!(5), json!({})] {
            assert_eq!(clean_name(&v), "A guest", "{v}");
        }
        // JS's trim, not Rust's: U+0085 stays (it is no JS white space), U+FEFF and U+3000 go
        assert_eq!(clean_name(&json!("\u{feff}\u{3000}Ana\u{2028}")), "Ana");
        assert_eq!(clean_name(&json!("\u{85}Ana")), "\u{85}Ana");
    }

    #[test]
    fn parse_doc_the_apis_answer_odd_fields_read_as_no() {
        let d = parse_doc(&json!({
            "available": true, "room": ROOM, "alias": "#guests:maple.example", "on": true, "since": 9,
            "link": "https://maple.example/call/?g=abcdefghijklmnopqrstuvwx", "waiting": [{ "id": "abcd1234", "name": "Ana", "at": 3 }],
            "canManage": true, "canAdmit": true, "member": true
        }));
        assert_eq!(
            d,
            Some(GuestsDoc {
                available: true,
                room: Some(ROOM.into()),
                alias: "#guests:maple.example".into(),
                on: true,
                since: 9.0,
                link: Some("https://maple.example/call/?g=abcdefghijklmnopqrstuvwx".into()),
                waiting: vec![w("abcd1234", "Ana", 3.0)],
                can_manage: true,
                can_admit: true,
                member: true,
            })
        );
        assert_eq!(
            parse_doc(&json!({ "available": true, "room": null, "alias": "#guests:x", "on": false, "since": 0, "link": null, "waiting": [], "canManage": false, "canAdmit": false, "member": false })),
            Some(GuestsDoc { available: true, room: None, alias: "#guests:x".into(), on: false, since: 0.0, link: None, waiting: vec![], can_manage: false, can_admit: false, member: false })
        );
        assert_eq!(
            parse_doc(&json!({ "room": "guests", "link": "javascript:x", "on": "yes", "canManage": 1, "since": -3 })),
            Some(GuestsDoc { available: false, room: None, alias: String::new(), on: false, since: 0.0, link: None, waiting: vec![], can_manage: false, can_admit: false, member: false })
        );
        for v in [Value::Null, json!([]), json!("x"), json!(3)] {
            assert_eq!(parse_doc(&v), None, "{v}");
        }
        // the regexes' edges (/^![^:\s]+:\S+$/, /^https?:\/\/\S+$/)
        let room = |r: &str| parse_doc(&json!({ "room": r })).unwrap().room;
        assert_eq!(room("!a:b:8448"), Some("!a:b:8448".into()));
        for bad in ["!:b", "!a:", "!a b:c", "!a:b c", "a:b", ""] {
            assert_eq!(room(bad), None, "{bad}");
        }
        let link = |l: &str| parse_doc(&json!({ "link": l })).unwrap().link;
        assert_eq!(link("http://alpha.local/call/?g=x"), Some("http://alpha.local/call/?g=x".into()));
        for bad in ["https://", "https://a b", "ftp://x", "HTTPS://x"] {
            assert_eq!(link(bad), None, "{bad}");
        }
    }

    #[test]
    fn is_guest_account_the_servers_temporary_accounts_only() {
        assert!(is_guest_account("@guest-k3j2h1abcd:maple.example"));
        assert!(is_guest_account("@guest-:maple.example"));
        for v in ["@guests:maple.example", "@ana:maple.example", "@guestbook:x", "guest-x:y", "", "@guest-x"] {
            assert!(!is_guest_account(v), "{v}");
        }
    }

    #[test]
    fn local_role_the_servers_rule_from_the_rooms_power_levels() {
        let pl = json!({ "users": { ME: 50, "@mod:maple.example": 100 }, "users_default": 0, "invite": 0 });
        assert_eq!(local_role(ME, Some("join"), &pl), role(true, false, true, true));
        assert_eq!(local_role("@bo:maple.example", Some("join"), &pl), role(true, false, true, false));
        assert_eq!(local_role("@bo:maple.example", Some("leave"), &pl), role(false, false, false, false));
        assert_eq!(local_role("@bo:maple.example", Some("invite"), &pl), role(false, false, false, false));
        assert_eq!(
            local_role("@guest-abc:maple.example", Some("join"), &json!({ "users": { "@guest-abc:maple.example": 100 } })),
            role(true, true, false, false),
            "a guest never"
        );
        // invite raised above the member's level: no admitting
        assert!(!local_role("@bo:maple.example", Some("join"), &json!({ "invite": 10 })).can_admit);
        // users_default carries; a non-integer level counts as unset
        assert!(local_role("@bo:maple.example", Some("join"), &json!({ "users_default": 50 })).can_manage);
        assert!(!local_role("@bo:maple.example", Some("join"), &json!({ "users": { "@bo:maple.example": 50.5 } })).can_manage);
        assert!(local_role("@bo:maple.example", Some("join"), &json!({ "invite": "5" })).can_admit, "odd invite level = 0");
        // no power levels at all: everyone 0, invite 0
        assert_eq!(local_role("@bo:maple.example", Some("join"), &Value::Null), role(true, false, true, false));
        // no membership known (not in the room)
        assert_eq!(local_role("@bo:maple.example", None, &Value::Null), role(false, false, false, false));
    }

    #[test]
    fn merge_role_either_side_may_say_yes_a_guest_never() {
        let out = role(false, false, false, false);
        assert_eq!(merge_role(out, None), out);
        assert_eq!(merge_role(out, Some(DocRole { member: false, can_admit: false, can_manage: true })), role(false, false, false, true), "a server admin outside the room");
        assert_eq!(merge_role(MEMBER, Some(DocRole { member: false, can_admit: false, can_manage: false })), MEMBER, "sync is fresher than the doc's cache");
        assert_eq!(merge_role(role(true, true, false, false), Some(DocRole { member: true, can_admit: true, can_manage: true })), role(true, true, false, false));
    }

    #[test]
    fn bar_plan_the_words_and_buttons_by_role() {
        use BarButton::*;
        assert_eq!(bar_plan(true, MANAGER), Some(BarPlan { text: "Guest room is on", buttons: vec![Copy, NewLink, TurnOff] }));
        assert_eq!(bar_plan(true, MEMBER), Some(BarPlan { text: "Guest room is on", buttons: vec![Copy] }));
        assert_eq!(bar_plan(false, MANAGER), Some(BarPlan { text: "Guest room is off", buttons: vec![TurnOn] }));
        assert_eq!(bar_plan(false, MEMBER), Some(BarPlan { text: "The guest room is off", buttons: vec![] }));
        assert_eq!(bar_plan(true, GUEST), None, "guests see no bar");
        assert_eq!(bar_plan(false, GUEST), None);
        assert_eq!(bar_plan(true, OUTSIDER), None);
        assert_eq!(bar_plan(true, role(false, false, false, true)), Some(BarPlan { text: "Guest room is on", buttons: vec![Copy, NewLink, TurnOff] }), "a server admin");
    }

    #[test]
    fn waiting_rows_while_on_for_those_who_may_let_people_in() {
        let ws = vec![w("abcd1234", "Ana", 1.0), w("efgh5678", "Bo", 2.0)];
        assert_eq!(waiting_rows(true, MEMBER, &ws), ws);
        assert_eq!(waiting_rows(true, MANAGER, &ws), ws);
        assert_eq!(waiting_rows(false, MANAGER, &ws), NONE, "off: nobody waits");
        assert_eq!(waiting_rows(true, GUEST, &ws), NONE, "guests never see who waits");
        assert_eq!(waiting_rows(true, OUTSIDER, &ws), NONE);
        assert_eq!(waiting_rows(true, role(false, false, false, true), &ws), NONE, "a server admin outside the room: join it to let people in");
    }

    #[test]
    fn pane_plan_settings_guests_by_state_and_role() {
        use PaneButton::*;
        let plan = |state: PaneState, text: &'static str, buttons: Vec<PaneButton>, show_link: bool| PanePlan { state, text, buttons, show_link };
        assert_eq!(pane_plan(&GuestsDoc { available: false, ..doc() }, MANAGER), plan(PaneState::Unavailable, "The guest room isn't available on this server.", vec![], false));
        let never = GuestsDoc { room: None, on: false, link: None, ..doc() };
        assert_eq!(pane_plan(&never, MANAGER), plan(PaneState::Never, "The guest room hasn't been turned on yet.", vec![TurnOn], false));
        assert_eq!(pane_plan(&never, OUTSIDER), plan(PaneState::Never, "The guest room hasn't been turned on yet. A server admin can turn it on here.", vec![], false));
        assert_eq!(pane_plan(&doc(), MANAGER), plan(PaneState::On, "The guest room is on.", vec![Copy, NewLink, TurnOff, Open], true));
        assert_eq!(pane_plan(&doc(), MEMBER), plan(PaneState::On, "The guest room is on.", vec![Copy, Open], true));
        let no_link = GuestsDoc { link: None, ..doc() };
        assert_eq!(pane_plan(&no_link, OUTSIDER), plan(PaneState::On, "The guest room is on.", vec![Open], false));
        assert_eq!(pane_plan(&no_link, MEMBER), plan(PaneState::On, "The guest room is on.", vec![Copy, Open], false), "no link in the doc yet: the button fetches it");
        let off = GuestsDoc { on: false, link: None, ..doc() };
        assert_eq!(pane_plan(&off, MANAGER), plan(PaneState::Off, "The guest room is off.", vec![TurnOn, Open], false));
        assert_eq!(pane_plan(&off, MEMBER), plan(PaneState::Off, "The guest room is off.", vec![Open], false));
        assert_eq!(pane_plan(&doc(), GUEST), plan(PaneState::On, "The guest room is on.", vec![], false));
        assert_eq!([PaneState::Unavailable, PaneState::Never, PaneState::On, PaneState::Off].map(PaneState::name), ["unavailable", "never", "on", "off"]);
    }

    #[test]
    fn is_guests_room_the_docs_room_before_it_the_servers_alias_with_the_state() {
        let alias = Some("#guests:maple.example");
        let server = "maple.example";
        assert!(is_guests_room(ROOM, Some(ROOM), true, alias, server));
        assert!(!is_guests_room(ROOM, Some("!other:maple.example"), true, alias, server), "the doc wins");
        assert!(is_guests_room(ROOM, Some(ROOM), false, None, server));
        assert!(is_guests_room(ROOM, None, true, alias, server));
        assert!(!is_guests_room(ROOM, None, false, alias, server));
        assert!(!is_guests_room(ROOM, None, true, Some("#guests:other.example"), server));
        assert!(!is_guests_room(ROOM, None, true, Some("#lounge:maple.example"), server), "someone's own room with a forged state event");
        assert!(!is_guests_room(ROOM, None, true, alias, ""));
        assert!(is_guests_room(ROOM, Some(""), true, alias, server), "an empty doc room is no doc room (JS falsy)");
    }

    #[test]
    fn arrivals_each_id_once_per_device_only_while_eligible_never_a_stale_knock() {
        let now = 10_000_000.0;
        let ana = w("abcd1234", "Ana", now - 1000.0);
        let bo = w("efgh5678", "Bo", now - 500.0);
        let r = arrivals(&[], &[ana.clone()], now, true);
        assert_eq!(r, Arrivals { notify: vec![ana.clone()], seen: ids(&["abcd1234"]) });
        // the same list again (a re-sync, the event echoed, a restart with the stored ids): nothing
        let r = arrivals(&r.seen, &[ana.clone()], now + 10.0, true);
        assert_eq!(r, Arrivals { notify: vec![], seen: ids(&["abcd1234"]) });
        // a second guest knocks
        let r = arrivals(&r.seen, &[ana.clone(), bo.clone()], now + 20.0, true);
        assert_eq!(r, Arrivals { notify: vec![bo.clone()], seen: ids(&["abcd1234", "efgh5678"]) });
        // Ana let in, Bo still waits, Ana knocks again with a new id
        let ana2 = w("ijkl9012", "Ana", now + 30.0);
        let r = arrivals(&r.seen, &[bo.clone(), ana2.clone()], now + 40.0, true);
        assert_eq!(r.notify, vec![ana2]);
        // not eligible (a guest, not a member, the feature off): nothing, nothing remembered …
        let off = arrivals(&[], &[ana.clone()], now, false);
        assert_eq!(off, Arrivals { notify: vec![], seen: vec![] });
        // … so becoming eligible while they still wait announces them once
        assert_eq!(arrivals(&off.seen, &[ana.clone()], now, true).notify, vec![ana.clone()]);
        // a knock older than 10 minutes: remembered, never announced
        let old = w("mnop3456", "Cy", now - WAIT_MS);
        assert_eq!(arrivals(&[], &[old], now, true), Arrivals { notify: vec![], seen: ids(&["mnop3456"]) });
        // no time given: announced
        let untimed = w("qrst7890", "Di", 0.0);
        assert_eq!(arrivals(&[], &[untimed.clone()], now, true).notify, vec![untimed]);
        // the memory is bounded, newest kept
        let seen: Vec<String> = (0..SEEN_MAX).map(|i| format!("s{i:07}")).collect();
        let r2 = arrivals(&seen, &[ana], now, true);
        assert_eq!(r2.seen.len(), SEEN_MAX);
        assert_eq!(r2.seen[SEEN_MAX - 1], "abcd1234");
        assert_eq!(r2.seen[0], "s0000001");
    }

    #[test]
    fn departures_whoever_left_the_list() {
        let ws = vec![w("efgh5678", "Bo", 1.0)];
        assert_eq!(departures(&ids(&["abcd1234", "efgh5678"]), &ws), ids(&["abcd1234"]));
        assert_eq!(departures(&[], &ws), Vec::<String>::new());
        assert_eq!(departures(&ids(&["abcd1234"]), &[]), ids(&["abcd1234"]));
    }

    #[test]
    fn account_name_a_guests_display_name_everyone_elses_localpart() {
        assert_eq!(account_name("@guest-9b230bc1ab:alpha.local", Some("Cousin Jo (guest)")), "Cousin Jo (guest)");
        assert_eq!(account_name("@guest-9b230bc1ab:alpha.local", Some("  Cousin Jo (guest) ")), "Cousin Jo (guest)");
        assert_eq!(account_name("@guest-9b230bc1ab:alpha.local", Some("")), "guest-9b230bc1ab", "no name: the localpart");
        assert_eq!(account_name("@guest-9b230bc1ab:alpha.local", None), "guest-9b230bc1ab");
        assert_eq!(account_name("@guest-9b230bc1ab:alpha.local", Some("   ")), "guest-9b230bc1ab");
        assert_eq!(account_name("@ana:alpha.local", Some("Ana Smith")), "ana", "members keep the localpart");
        assert_eq!(account_name("@ana:alpha.local", None), "ana");
    }
}
