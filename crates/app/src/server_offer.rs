//! server_offer.rs — what the user's Tin Hat Server offers (selfhost, 2026-10-21: nothing server-specific hard-coded
//! in the app). SHARED LOGIC with Tin Hat Web's `src/core/server/offer.ts` (the rules, line for line) and
//! `store.ts` (when it is read, what is kept); the unit tests below replay offer.test.ts's cases.
//!
//! Discovery: `GET https://<server name>/.well-known/matrix/client` carries
//!   "dev.tinhat.server": {"server": "0.2.21", "api": "https://<host>/api",
//!                        "features": {"calls", "soundboard", "gifs", "watch", "games", "steam", "gameservers",
//!                                     "presence", "push", "memories", "reports", "guests"},
//!                        "recorder": "@recorder:<server>",   // only while memories is on: the Memories recorder
//!                        "account": {"password_reset": "owner"}}   // no email here: the owner sets new passwords
//! plus the standard `org.matrix.msc4143.rtc_foci` (the call service; rtc.rs focus_from_wellknown).
//!
//! The rules: a feature is OFF only when the server says `false`. No document, a 404, no key, no `features`, or a
//! value that is not a boolean = on (today's behaviour). A network error, another status or a body that isn't JSON
//! keeps the last answer. Read at sign-in, at restore and when the window comes back (at most every 10 min), kept
//! in the settings store (`blob-server-offer`, forgotten at sign-out) so a restart draws the right buttons at once.
//! Off means hidden, never dimmed: the actions a feature carries get the contract's `requires` rule through the
//! global predicate `server.<feature>` (apply_action_gates, laid over the table at start; ActionsHost.qml feeds the
//! values); the settings rows it carries leave the Settings search; widgets leave the gallery and the stack; roles
//! rows leave the editor; everything else asks `feature_on` (QML: `app.serverOn("…")`).
//!
//! What else the offer decides: the API base (`api`; proxy.rs), the call service (rtc.rs), the recorder's account
//! (room.rs recorder_mxid: call tiles, the REC indicator, call alerts and the streamer's receipts filter it out;
//! with no recorder nothing is filtered), how a forgotten password comes back (`account.password_reset`: `"owner"` =
//! this server sends no email, its owner sets a new password from the dashboard; missing or any other value = the
//! email form; password.rs — the sign-in screen asks the typed server itself, this store keeps the signed-in one's).
//!
//! Retired whatever the server offers (`RETIRED_ACTIONS`, the always-false global `selfhost.retired`): the per-room
//! guest link — the guest room (`guests`, docs/GUESTS.md in the server repo) replaces it. A temporary guest account
//! (`@guest-…`) never gets `GUEST_WITHHELD` (the server refuses its reports).
//!
//! `TINHAT_SERVER_OFFER` (JSON, the `dev.tinhat.server` value) pins an offer for gates and screenshots: the server is
//! never asked.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::password::PasswordReset;

/// The well-known key a Tin Hat Server announces itself under.
pub const DISCOVERY_KEY: &str = "dev.tinhat.server";

/// Everything a server can offer beyond chat, in the server's order (apiconf.py `features()`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ServerFeature {
    Calls,
    Soundboard,
    Gifs,
    Watch,
    Games,
    Steam,
    Gameservers,
    Presence,
    Push,
    Memories,
    Reports,
    Guests,
}

pub const SERVER_FEATURES: [ServerFeature; 12] = [
    ServerFeature::Calls,
    ServerFeature::Soundboard,
    ServerFeature::Gifs,
    ServerFeature::Watch,
    ServerFeature::Games,
    ServerFeature::Steam,
    ServerFeature::Gameservers,
    ServerFeature::Presence,
    ServerFeature::Push,
    ServerFeature::Memories,
    ServerFeature::Reports,
    ServerFeature::Guests,
];

impl ServerFeature {
    /// The key in the announcement (and in `server.<name>`).
    pub fn name(self) -> &'static str {
        match self {
            ServerFeature::Calls => "calls",
            ServerFeature::Soundboard => "soundboard",
            ServerFeature::Gifs => "gifs",
            ServerFeature::Watch => "watch",
            ServerFeature::Games => "games",
            ServerFeature::Steam => "steam",
            ServerFeature::Gameservers => "gameservers",
            ServerFeature::Presence => "presence",
            ServerFeature::Push => "push",
            ServerFeature::Memories => "memories",
            ServerFeature::Reports => "reports",
            ServerFeature::Guests => "guests",
        }
    }

    /// What the feature covers in the apps (offer.ts FEATURE_DOC, byte for byte).
    pub fn doc(self) -> &'static str {
        match self {
            ServerFeature::Calls => "calls (a call service on the server)",
            ServerFeature::Soundboard => "the soundboard (and the join sounds it hosts)",
            ServerFeature::Gifs => "the GIF picker",
            ServerFeature::Watch => "YouTube search and browsing in Media Party (pasted links always work)",
            ServerFeature::Games => "the Game Reel's games catalogue (search, add by name, covers)",
            ServerFeature::Steam => "importing a Steam library into My games",
            ServerFeature::Gameservers => "the Game servers widget",
            ServerFeature::Presence => "Reach: online / away / in-a-call statuses, custom statuses, Who's around, Friends' clocks, Is it just me?",
            ServerFeature::Push => "push notifications on phones and closed browsers",
            ServerFeature::Memories => "Memories, the call recorder (and its recorder account)",
            ServerFeature::Reports => "Report a problem",
            ServerFeature::Guests => "the guest room (Settings → Guests, the Guests room's bar, the waiting-at-the-door notices — docs/GUESTS.md)",
        }
    }

    pub fn parse(s: &str) -> Option<ServerFeature> {
        SERVER_FEATURES.iter().copied().find(|f| f.name() == s)
    }
}

/// A server's announcement, as much of it as counts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerOffer {
    /// the server's version ("0.2.21"); "" when it doesn't say
    pub server: String,
    /// the Tin Hat API base it announces (no trailing slash); "" when it doesn't say
    pub api: String,
    /// only the features it announced as booleans
    pub features: BTreeMap<ServerFeature, bool>,
    /// the Memories recorder's account, as announced ("" when it doesn't say); `recorder()` applies the Memories rule
    pub recorder: String,
    /// `account.password_reset`: Owner only for the string "owner" (password.rs)
    pub password_reset: PasswordReset,
}

/// No announcement (today's behaviour everywhere).
pub fn no_offer() -> ServerOffer {
    ServerOffer::default()
}

fn obj(v: &Value) -> Option<&Map<String, Value>> {
    v.as_object()
}

/// JS `\s` (the web's regexes): Unicode White_Space plus U+FEFF.
fn js_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// A plain user id: the web's `MXID = /^@[^:\s]+:[^\s]+$/` (offer.ts).
fn is_mxid(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('@') else { return false };
    let Some((local, server)) = rest.split_once(':') else { return false };
    !local.is_empty() && !local.contains(js_space) && !server.is_empty() && !server.contains(js_space)
}

/// An API base as announced: an http(s) URL, without its trailing slash; anything else is "no base".
fn api_base_of(s: &str) -> String {
    let s = s.trim();
    if s.starts_with("https://") || s.starts_with("http://") {
        s.trim_end_matches('/').to_string()
    } else {
        String::new()
    }
}

/// The `dev.tinhat.server` part of a /.well-known/matrix/client answer (any JSON value; never fails).
pub fn parse_offer(well_known: &Value) -> ServerOffer {
    let Some(w) = obj(well_known) else { return no_offer() };
    let Some(t) = w.get(DISCOVERY_KEY).and_then(obj) else { return no_offer() };
    let server = t.get("server").and_then(Value::as_str).unwrap_or("").to_string();
    let api = t.get("api").and_then(Value::as_str).map(api_base_of).unwrap_or_default();
    let mut features = BTreeMap::new();
    if let Some(f) = t.get("features").and_then(obj) {
        for k in SERVER_FEATURES {
            if let Some(Value::Bool(b)) = f.get(k.name()) {
                features.insert(k, *b);
            }
        }
    }
    // the recorder only counts while Memories is on (the server sends it only then; a stray value is ignored)
    let memories_off = features.get(&ServerFeature::Memories) == Some(&false);
    let recorder = t.get("recorder").and_then(Value::as_str).filter(|r| is_mxid(r) && !memories_off).unwrap_or("").to_string();
    // a forgotten password: only the string "owner" (anything else, or no `account` object, is the email form)
    let password_reset = match t.get("account").and_then(obj).and_then(|a| a.get("password_reset")).and_then(Value::as_str) {
        Some("owner") => PasswordReset::Owner,
        _ => PasswordReset::Email,
    };
    ServerOffer { server, api, features, recorder, password_reset }
}

/// Is the feature on? Only an explicit `false` turns it off.
pub fn feature_on(offer: &ServerOffer, f: ServerFeature) -> bool {
    offer.features.get(&f) != Some(&false)
}

/// What a temporary guest account (`@guest-…`, docs/GUESTS.md) never gets, whatever the server offers: the server
/// refuses its problem reports and shared logs (403).
pub const GUEST_WITHHELD: [ServerFeature; 1] = [ServerFeature::Reports];

/// Is the feature on for this user? `feature_on`, minus GUEST_WITHHELD for a guest account (offer.ts featureFor).
pub fn feature_for(offer: &ServerOffer, f: ServerFeature, guest: bool) -> bool {
    feature_on(offer, f) && !(guest && GUEST_WITHHELD.contains(&f))
}

/// Settings rows a temporary guest account never sees, whatever the server offers (they leave the Settings search too;
/// SettingsWindow hides them by `app.mGuestAccount`): its password is the server's, not one it chose.
pub const GUEST_HIDDEN_SETTINGS: [&str; 1] = ["security.password"];

/// Is this settings row shown to this user? (GUEST_HIDDEN_SETTINGS for a guest account.)
pub fn setting_for_guest(id: &str, guest: bool) -> bool {
    !(guest && GUEST_HIDDEN_SETTINGS.contains(&id))
}

/// A temporary guest account (`@guest-…`, made by the server for whoever was let in — the guest room's, and the old
/// per-room links'): no encryption set-up, no verify prompts, none of the guest room's buttons. offer.ts
/// `isGuestAccount` = `/^@guest-[^:]*:./` (`.` = any character but a line terminator).
pub fn is_guest_account(mxid: &str) -> bool {
    let Some(rest) = mxid.strip_prefix("@guest-") else { return false };
    let Some((_, after)) = rest.split_once(':') else { return false };
    after.chars().next().is_some_and(|c| !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'))
}

/// Is `id` the recorder? `id` is a Matrix id ("@u:server") or a LiveKit identity ("@u:server:DEVICE"); `recorder` is
/// the announced account ("" = none, so nobody is). offer.ts `isRecorderId`.
pub fn is_recorder_id(id: &str, recorder: &str) -> bool {
    if recorder.is_empty() || id.is_empty() {
        return false;
    }
    id == recorder || id.strip_prefix(recorder).is_some_and(|r| r.starts_with(':'))
}

/// The same offer? (the store notifies only on a real change)
pub fn same_offer(a: &ServerOffer, b: &ServerOffer) -> bool {
    a == b
}

/// The offer as the store keeps it and QML reads it: `{server, api, features: {...}, recorder}`, plus
/// `account: {password_reset: "owner"}` when the server said so (the document's own shape; absent = the email form).
pub fn offer_to_json(o: &ServerOffer) -> Value {
    let mut f = Map::new();
    for (k, v) in &o.features {
        f.insert(k.name().to_string(), Value::Bool(*v));
    }
    let mut v = json!({ "server": o.server, "api": o.api, "features": f, "recorder": o.recorder });
    if o.password_reset == PasswordReset::Owner {
        v["account"] = json!({ "password_reset": PasswordReset::Owner.name() });
    }
    v
}

// ---- the actions a feature carries (actions.json ids; ACTIONS.md) -----------------------------------------------

/// id → feature. An entry ending in "." covers every id with that prefix.
pub const ACTION_GATES: &[(&str, ServerFeature)] = &[
    // Memories = the call recorder and its library
    ("memories.", ServerFeature::Memories),
    ("room.recording.toggle", ServerFeature::Memories),
    ("call.clip", ServerFeature::Memories),
    ("call.share.recordingOnly.toggle", ServerFeature::Memories),
    ("call.recStream.tuneIn", ServerFeature::Memories),
    ("call.recStream.leave", ServerFeature::Memories),
    ("app.recordOptOut.toggle", ServerFeature::Memories),
    // the soundboard (and the join sounds it hosts)
    ("soundboard.", ServerFeature::Soundboard),
    ("sound.", ServerFeature::Soundboard),
    ("person.joinSound.play", ServerFeature::Soundboard),
    // the GIF picker (Giphy, the server's key)
    ("composer.gif", ServerFeature::Gifs),
    // Reach: your status (status.open / set / clear), the one-time question, the sharing switch
    ("status.", ServerFeature::Presence),
    ("presence.", ServerFeature::Presence),
    // push notifications while Tin Hat is closed (push.enable / quiet.toggle / test; none on the desktop)
    ("push.", ServerFeature::Push),
    // Report a problem, and sharing logs (the server's same reports / logs inbox)
    ("app.bugReport", ServerFeature::Reports),
    ("app.logship.toggle", ServerFeature::Reports),
];

/// The feature an action needs, or None.
pub fn gate_of_action(id: &str) -> Option<ServerFeature> {
    for (k, f) in ACTION_GATES {
        let hit = if k.ends_with('.') { id.starts_with(k) } else { id == *k };
        if hit {
            return Some(*f);
        }
    }
    None
}

/// The global predicate that carries a feature into the actions engine's snapshot.
pub fn feature_predicate(f: ServerFeature) -> String {
    format!("server.{}", f.name())
}

// ---- actions the desktop doesn't have at all (whatever the server offers) ----------------------------------------

/// Replaced in the self-hosted builds of both apps (the desktop has only that one): the per-room guest link
/// (a room opened to browser guests, auto-admit) → the guest room (docs/GUESTS.md). offer.ts
/// RETIRED_ACTIONS.
pub const RETIRED_ACTIONS: [&str; 2] = ["room.guestLink", "room.guestLink.disable"];
/// The global predicate that hides them: always false in the snapshot.
pub const RETIRED_PREDICATE: &str = "selfhost.retired";

pub fn is_retired_action(id: &str) -> bool {
    RETIRED_ACTIONS.contains(&id)
}

/// The predicate values for the actions snapshot (`values`): one per feature (for a guest account: feature_for) + the
/// retired actions' (false). offer.ts predicateValues.
pub fn predicate_values(offer: &ServerOffer, guest: bool) -> Map<String, Value> {
    let mut out = Map::new();
    for f in SERVER_FEATURES {
        out.insert(feature_predicate(f), Value::Bool(feature_for(offer, f, guest)));
    }
    out.insert(RETIRED_PREDICATE.to_string(), Value::Bool(false));
    out
}

fn predicate_doc(f: ServerFeature) -> String {
    format!("The server offers {} (a self-hosted Tin Hat Server's announcement; absent = yes).", f.doc())
}

/// Lay the gates over a loaded actions table (once at start): every gated action gets `requires` = its own rule AND
/// `server.<feature>` (a requires-less action: just the predicate), every retired action the same with
/// `selfhost.retired`, and the predicates are declared as global booleans. Takes the table JSON (`{actions,
/// predicates, …}` as actions.json; a `{"table": {...}}` wrapper is looked through). Idempotent; returns how many
/// actions it changed. offer.ts applyActionGates.
pub fn apply_action_gates(table: &mut Value) -> usize {
    let t = if table.get("table").is_some_and(Value::is_object) { &mut table["table"] } else { table };
    let Some(t) = t.as_object_mut() else { return 0 };
    let preds = t.entry("predicates").or_insert_with(|| Value::Object(Map::new()));
    if let Some(preds) = preds.as_object_mut() {
        for f in SERVER_FEATURES {
            let p = feature_predicate(f);
            if !preds.contains_key(&p) {
                preds.insert(p, json!({ "type": "bool", "scope": "global", "doc": predicate_doc(f) }));
            }
        }
        if !preds.contains_key(RETIRED_PREDICATE) {
            preds.insert(
                RETIRED_PREDICATE.to_string(),
                json!({ "type": "bool", "scope": "global", "doc": "Always false in a self-hosted build: actions it replaced (the per-room guest link → the guest room)." }),
            );
        }
    }
    let mut n = 0;
    let Some(actions) = t.get_mut("actions").and_then(Value::as_array_mut) else { return 0 };
    for a in actions.iter_mut() {
        let Some(id) = a.get("id").and_then(Value::as_str) else { continue };
        let p = match gate_of_action(id) {
            Some(f) => feature_predicate(f),
            None if is_retired_action(id) => RETIRED_PREDICATE.to_string(),
            None => continue,
        };
        let r = a.get("requires").cloned();
        if r.as_ref().and_then(Value::as_str) == Some(p.as_str()) {
            continue;
        }
        if r.as_ref().and_then(|r| r.get("all")).and_then(Value::as_array).is_some_and(|all| all.iter().any(|x| x.as_str() == Some(p.as_str()))) {
            continue;
        }
        let next = match r {
            None | Some(Value::Null) => Value::String(p),
            Some(r) => json!({ "all": [r, p] }),
        };
        if let Some(o) = a.as_object_mut() {
            o.insert("requires".into(), next);
            n += 1;
        }
    }
    n
}

// ---- Settings rows a feature carries (settings_index.json ids) --------------------------------------------------

pub const SETTING_GATES: &[(&str, ServerFeature)] = &[
    ("privacy.never-record", ServerFeature::Memories),
    ("profile.join-sound", ServerFeature::Soundboard),
    // Reach: "Your status" and the time zone Friends' clocks shows (both live in the presence service)
    ("privacy.share-status", ServerFeature::Presence),
    ("privacy.share-time-zone", ServerFeature::Presence),
    ("privacy.time-zone", ServerFeature::Presence),
    ("about.report", ServerFeature::Reports),
    ("privacy.log-sharing", ServerFeature::Reports),
    // the guest room's pane (Settings → Guests: its state + the link)
    ("guests.state", ServerFeature::Guests),
    ("guests.link", ServerFeature::Guests),
];

pub fn gate_of_setting(id: &str) -> Option<ServerFeature> {
    SETTING_GATES.iter().find(|(k, _)| *k == id).map(|(_, f)| *f)
}

// ---- widgets a feature carries (widgets.json ids) and roles permissions (roles_model.rs keys) --------------------

pub const WIDGET_GATES: &[(&str, ServerFeature)] = &[
    ("memories", ServerFeature::Memories),
    ("soundPad", ServerFeature::Soundboard),
    ("gameServers", ServerFeature::Gameservers),
    // Reach's widgets: who shares a status, their clocks, and the probe of the presence service
    ("whosAround", ServerFeature::Presence),
    ("clocks", ServerFeature::Presence),
    ("justMe", ServerFeature::Presence),
];

pub fn gate_of_widget(id: &str) -> Option<ServerFeature> {
    WIDGET_GATES.iter().find(|(k, _)| *k == id).map(|(_, f)| *f)
}

pub const PERMISSION_GATES: &[(&str, ServerFeature)] = &[
    ("record_calls", ServerFeature::Memories),
    ("use_soundboard", ServerFeature::Soundboard),
    ("manage_soundboard", ServerFeature::Soundboard),
];

pub fn gate_of_permission(key: &str) -> Option<ServerFeature> {
    PERMISSION_GATES.iter().find(|(k, _)| *k == key).map(|(_, f)| *f)
}

// ---- Settings panes a feature carries (SettingsWindow.qml pane keys) ----------------------------------------------

pub const PANE_GATES: &[(&str, ServerFeature)] = &[("guests", ServerFeature::Guests)];

pub fn gate_of_pane(key: &str) -> Option<ServerFeature> {
    PANE_GATES.iter().find(|(k, _)| *k == key).map(|(_, f)| *f)
}

/// Every gate table, for QML (`login.serverGatesJson()`): `{features: [...], actions: [[id, feature]…],
/// settings: {id: feature}, widgets: {id: feature}, permissions: {key: feature}, panes: {key: feature},
/// retired: [...], retiredPredicate, guestWithheld: [...]}`.
pub fn gates_json() -> String {
    let map = |t: &[(&str, ServerFeature)]| -> Value {
        let mut m = Map::new();
        for (k, f) in t {
            m.insert((*k).to_string(), Value::String(f.name().to_string()));
        }
        Value::Object(m)
    };
    json!({
        "features": SERVER_FEATURES.iter().map(|f| f.name()).collect::<Vec<_>>(),
        "actions": ACTION_GATES.iter().map(|(k, f)| json!([k, f.name()])).collect::<Vec<_>>(),
        "settings": map(SETTING_GATES),
        "widgets": map(WIDGET_GATES),
        "permissions": map(PERMISSION_GATES),
        "panes": map(PANE_GATES),
        "retired": RETIRED_ACTIONS,
        "retiredPredicate": RETIRED_PREDICATE,
        "guestWithheld": GUEST_WITHHELD.iter().map(|f| f.name()).collect::<Vec<_>>(),
    })
    .to_string()
}

// ---- the actions engine's table ----------------------------------------------------------------------------------

/// Lay the gates over the engine's table before its first use (actions_ctl calls it before every engine call, main()
/// at start). True when the table carries the gates (installed now or earlier); false when the engine had already
/// built its table without them (logged once).
pub fn ensure_actions_overlay() -> bool {
    static DONE: OnceLock<bool> = OnceLock::new();
    *DONE.get_or_init(|| {
        let ok = tinhat_actions::table::install_overlay(|v| {
            apply_action_gates(v);
        });
        if !ok {
            eprintln!("[server-offer] the actions table was built before the server gates could be laid over it");
        }
        ok
    })
}

// ---- the store: the offer at run time (store.ts) -----------------------------------------------------------------

/// The settings-store file (`<config>/tinhat/blob-server-offer`): the last answer, as the web keeps it in
/// localStorage (`tinhat.server-offer`); an account file, so sign-out forgets it (signout.rs).
pub const CACHE_PREF: &str = "blob-server-offer";
/// Ask again at most this often (store.ts REFRESH_MS).
pub const REFRESH: Duration = Duration::from_secs(10 * 60);

struct State {
    offer: ServerOffer,
    /// the call service the well-known names: Some(None) = a document read, none named; None = never read
    focus: Option<Option<String>>,
    /// the server name the answer belongs to
    server: String,
    last_read: Option<Instant>,
    in_flight: bool,
    /// TINHAT_SERVER_OFFER pinned it: the server is never asked
    pinned: bool,
    /// the names this server had before it moved to its own domain (`dev.tinhat.moved_from`; proxy.rs re-homes links
    /// on their API onto today's — the only other bases the app trusts)
    former: Vec<String>,
    /// bumped by forget(): a read that started before a sign-out never lands in the next account's state
    epoch: u64,
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();
/// Bumped on every change (QML mirrors `offer_json()` when it changes).
static REV: AtomicU64 = AtomicU64::new(1);

fn state() -> &'static Mutex<State> {
    STATE.get_or_init(|| {
        let (offer, focus, pinned) = match std::env::var("TINHAT_SERVER_OFFER").ok().filter(|s| !s.trim().is_empty()) {
            Some(s) => match serde_json::from_str::<Value>(&s) {
                Ok(v) => {
                    let o = parse_offer(&json!({ DISCOVERY_KEY: v }));
                    eprintln!("[server-offer] pinned by TINHAT_SERVER_OFFER: {}", offer_to_json(&o));
                    (o, None, true)
                }
                Err(e) => {
                    eprintln!("[server-offer] TINHAT_SERVER_OFFER is not JSON ({e}); ignored");
                    (no_offer(), None, false)
                }
            },
            None => {
                let (o, f) = read_cache();
                (o, f, false)
            }
        };
        let former = if pinned { Vec::new() } else { read_cache_former() };
        apply_side_effects(&offer);
        Mutex::new(State { offer, focus, server: String::new(), last_read: None, in_flight: false, pinned, former, epoch: 0 })
    })
}

fn lock() -> std::sync::MutexGuard<'static, State> {
    state().lock().unwrap_or_else(|e| e.into_inner())
}

/// The cache: `{"dev.tinhat.server": <offer as announced>, "org.matrix.msc4143.rtc_foci": [...]?}` — the shape of
/// the document it came from, so the parser is the one reader.
fn read_cache() -> (ServerOffer, Option<Option<String>>) {
    let Some(raw) = crate::settings::overlay_pref_read(CACHE_PREF) else { return (no_offer(), None) };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else { return (no_offer(), None) };
    let focus = v.get("org.matrix.msc4143.rtc_foci").map(|_| crate::rtc::focus_from_wellknown(&v));
    (parse_offer(&v), focus)
}

fn read_cache_former() -> Vec<String> {
    crate::settings::overlay_pref_read(CACHE_PREF)
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .map(|v| crate::moved::parse_move(&v).moved_from)
        .unwrap_or_default()
}

fn write_cache(o: &ServerOffer, focus: &Option<Option<String>>, former: &[String]) {
    if *o == no_offer() && focus.is_none() && former.is_empty() {
        let _ = std::fs::remove_file(crate::settings::overlay_pref_path(CACHE_PREF));
        return;
    }
    let mut doc = Map::new();
    let mut ann = offer_to_json(o);
    if o.recorder.is_empty() {
        if let Some(m) = ann.as_object_mut() {
            m.remove("recorder");
        }
    }
    doc.insert(DISCOVERY_KEY.into(), ann);
    if let Some(f) = focus {
        let foci = match f {
            Some(url) => json!([{ "type": "livekit", "livekit_service_url": url }]),
            None => json!([]),
        };
        doc.insert("org.matrix.msc4143.rtc_foci".into(), foci);
    }
    if !former.is_empty() {
        doc.insert("dev.tinhat.moved_from".into(), json!(former));
    }
    crate::settings::overlay_pref_write(CACHE_PREF, &Value::Object(doc).to_string());
}

/// What follows the offer around the process: the streamer's recorder identity.
fn apply_side_effects(o: &ServerOffer) {
    tinhat_streamer::hear::set_recorder(recorder_of(o));
}

/// May this session send its Matrix token to the API base the well-known announced? Only an https:// URL (http://
/// only when the homeserver itself is plain http on that host — a local test server) on the homeserver's own host or
/// the account's server name's host, with no user-info. Anything else is ignored and the app uses
/// `<homeserver origin>/api` (proxy.rs). A well-known can't send the bearer token to a third host.
pub fn api_allowed(api: &str, homeserver_url: &str, server_name: &str) -> bool {
    let Some((scheme, rest)) = api.split_once("://") else { return false };
    let hs = crate::session::origin_of(homeserver_url);
    let hs_scheme = hs.split("://").next().unwrap_or("");
    let ok_scheme = scheme == "https" || (scheme == "http" && hs_scheme == "http");
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if !ok_scheme || authority.is_empty() || authority.contains('@') || rest.contains(['?', '#']) {
        return false;
    }
    let host = authority.to_ascii_lowercase();
    let hs_host = hs.split_once("://").map(|(_, h)| h.to_string()).unwrap_or_default();
    let name = server_name.trim().to_ascii_lowercase();
    // the authority as written (host[:port]) must be the homeserver's, or the server name's (any port of it)
    let bare = |a: &str| a.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())).map(|(h, _)| h.to_string()).unwrap_or_else(|| a.to_string());
    !host.is_empty() && (host == hs_host || bare(&host) == bare(&hs_host) || bare(&host) == bare(&name))
}

fn recorder_of(o: &ServerOffer) -> Option<String> {
    if feature_on(o, ServerFeature::Memories) && !o.recorder.is_empty() {
        Some(o.recorder.clone())
    } else {
        None
    }
}

/// Keep a read answer (under the lock); true when something changed.
fn set(s: &mut State, server: &str, next: ServerOffer, focus: Option<Option<String>>) -> bool {
    let changed = !same_offer(&next, &s.offer) || s.focus != focus;
    s.server = server.to_string();
    if !changed {
        return false;
    }
    eprintln!(
        "[server-offer] {} → {} (server {}, api {}, recorder {:?}, call service {:?})",
        Value::Object(predicate_values(&s.offer, false)),
        Value::Object(predicate_values(&next, false)),
        if next.server.is_empty() { "?" } else { next.server.as_str() },
        if next.api.is_empty() { "-" } else { next.api.as_str() },
        recorder_of(&next),
        focus.clone().flatten()
    );
    s.offer = next;
    s.focus = focus;
    write_cache(&s.offer, &s.focus, &s.former);
    apply_side_effects(&s.offer);
    REV.fetch_add(1, Ordering::SeqCst);
    true
}

/// What the server offers (no offer before a server answered).
pub fn current() -> ServerOffer {
    lock().offer.clone()
}

/// The change counter (QML polls it).
pub fn rev() -> u64 {
    REV.load(Ordering::SeqCst)
}

/// The signed-in account is a temporary guest account (docs/GUESTS.md: some features are withheld from it).
pub fn guest_now() -> bool {
    crate::session::client().and_then(|c| c.user_id().map(|u| is_guest_account(u.as_str()))).unwrap_or(false)
}

/// Is this feature on for the signed-in account, process-wide? False only when the server said so (or, for a guest
/// account, a feature it never gets: GUEST_WITHHELD). store.ts serverFeatureOn.
pub fn feature_on_global(f: ServerFeature) -> bool {
    let guest = guest_now();
    feature_for(&lock().offer, f, guest)
}

/// The offer TINHAT_SERVER_OFFER pinned (gates, screenshots), if one did: the sign-in screen's "Forgot your password?"
/// takes its `account.password_reset` instead of asking the typed server (password.rs).
pub fn pinned_offer() -> Option<ServerOffer> {
    let s = lock();
    if s.pinned {
        Some(s.offer.clone())
    } else {
        None
    }
}

/// The Memories recorder's account, while the server offers Memories and names one.
pub fn recorder() -> Option<String> {
    recorder_of(&lock().offer)
}

/// The names the signed-in server had before it moved (`dev.tinhat.moved_from`; proxy.rs).
pub fn former_names() -> Vec<String> {
    lock().former.clone()
}

/// The API base the server announced (no trailing slash), if any.
pub fn api_base() -> Option<String> {
    let s = lock();
    if s.offer.api.is_empty() {
        None
    } else {
        Some(s.offer.api.clone())
    }
}

/// The call service of `server`'s last read well-known: Some(Some(url)) = it names one; Some(None) = a document
/// was read and names none; None = this server's document was never read (or the answer belongs to another).
pub fn focus_url_for(server: &str) -> Option<Option<String>> {
    let s = lock();
    if s.server == server || (s.server.is_empty() && s.focus.is_some()) {
        s.focus.clone()
    } else {
        None
    }
}

/// `{server, api, features, recorder, guest}` for QML (`login.serverOfferJson`): `guest` = the signed-in account is a
/// temporary guest account (QML's serverOn withholds GUEST_WITHHELD then, as feature_on_global).
pub fn offer_json() -> String {
    let guest = guest_now();
    let mut v = offer_to_json(&lock().offer);
    if let Some(m) = v.as_object_mut() {
        m.insert("guest".into(), Value::Bool(guest));
    }
    v.to_string()
}

/// Sign-out: the next account starts without the last server's answer (its cache file is an account file).
pub fn forget() {
    let mut s = lock();
    if s.pinned {
        return;
    }
    s.offer = no_offer();
    s.focus = None;
    s.server.clear();
    s.last_read = None;
    s.former.clear();
    // a read still on its way belongs to the account that just left: it must not land (refresh checks the epoch),
    // and it must not keep the next account's first read waiting
    s.epoch += 1;
    s.in_flight = false;
    let _ = std::fs::remove_file(crate::settings::overlay_pref_path(CACHE_PREF));
    apply_side_effects(&s.offer);
    REV.fetch_add(1, Ordering::SeqCst);
}

/// Test hook (gates, screenshots): lay an offer over the live one.
pub fn set_for_test(o: ServerOffer) {
    let mut s = lock();
    let server = s.server.clone();
    set(&mut s, &server, o, None);
}

enum Read {
    /// 200: the document (with or without our key)
    Doc(Value),
    /// 404: no discovery document (an older server) = no offer
    NotFound,
    /// a network error, another status, not JSON: keep the last answer
    Keep,
}

async fn read_well_known(server: &str) -> Read {
    let url = format!("https://{server}/.well-known/matrix/client");
    let resp = match crate::proxy::http(Some(Duration::from_secs(8))).get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[server-offer] couldn't read {url} ({e}) — keeping the last answer");
            return Read::Keep;
        }
    };
    let status = resp.status();
    if status.as_u16() == 404 {
        return Read::NotFound;
    }
    if !status.is_success() {
        eprintln!("[server-offer] {url}: HTTP {status} — keeping the last answer");
        return Read::Keep;
    }
    let text = resp.text().await.unwrap_or_default();
    match serde_json::from_str::<Value>(&text) {
        Ok(v) => Read::Doc(v),
        Err(e) => {
            eprintln!("[server-offer] {url}: not JSON ({e}) — keeping the last answer");
            Read::Keep
        }
    }
}

/// Ask the user's server again (at most every 10 min unless `force`): sign-in, restore, the window coming back, the
/// 10-minute ticker. The server name is the account's (`@user:server` → `https://server/.well-known/…`, the standard
/// place), never a built-in one. Returns true when the answer changed.
pub async fn refresh(client: &matrix_sdk::Client, force: bool) -> bool {
    let Some(server) = client.user_id().map(|u| u.server_name().to_string()) else { return false };
    let homeserver = client.homeserver().to_string();
    let epoch = {
        let mut s = lock();
        if s.pinned || s.in_flight {
            return false;
        }
        if !force && s.server == server && s.last_read.is_some_and(|t| t.elapsed() < REFRESH) {
            return false;
        }
        s.in_flight = true;
        s.last_read = Some(Instant::now());
        s.epoch
    };
    let outcome = read_well_known(&server).await;
    let mut s = lock();
    if s.epoch != epoch {
        // signed out while this read was on its way: the answer belongs to an account that is gone
        eprintln!("[server-offer] dropped an answer for {server}: signed out while it was on its way");
        return false;
    }
    s.in_flight = false;
    match outcome {
        Read::NotFound => {
            s.former.clear();
            set(&mut s, &server, no_offer(), Some(None))
        }
        Read::Doc(v) => {
            let focus = Some(crate::rtc::focus_from_wellknown(&v));
            let mut offer = parse_offer(&v);
            if !offer.api.is_empty() && !api_allowed(&offer.api, &homeserver, &server) {
                eprintln!("[server-offer] ignoring the announced api {} (not this server's host); using <homeserver>/api", offer.api);
                offer.api.clear();
            }
            let former = crate::moved::parse_move(&v).moved_from;
            let former_changed = former != s.former;
            s.former = former;
            let changed = set(&mut s, &server, offer, focus);
            if former_changed && !changed {
                write_cache(&s.offer, &s.focus, &s.former);
            }
            changed
        }
        Read::Keep => {
            s.last_read = None; // try again on the next occasion
            false
        }
    }
}

#[cfg(test)]
mod tests {
    //! offer.test.ts's cases (Tin Hat Web, branch selfhost), one test per web test where the web has one, plus the
    //! desktop's own: the store and the real engine through actions_ctl.
    use super::*;

    // the announcement Tin Hat Server 0.2.21 sends (docs/API.md's example, byte for byte: a Giphy key, no YouTube key,
    // no Steam key, no game-servers panel, Memories on with its recorder)
    fn announced() -> Value {
        json!({
            "m.homeserver": { "base_url": "https://maplehouse.tinhat.me" },
            "org.matrix.msc4143.rtc_foci": [{ "type": "livekit", "livekit_service_url": "https://maplehouse.tinhat.me/livekit/jwt" }],
            "dev.tinhat.server": {
                "server": "0.2.21",
                "api": "https://maplehouse.tinhat.me/api",
                "features": { "calls": true, "soundboard": true, "gifs": true, "watch": false, "games": true, "steam": false,
                              "gameservers": false, "presence": true, "push": true, "memories": true, "reports": true, "guests": true },
                "recorder": "@recorder:maplehouse.tinhat.me"
            }
        })
    }
    // an offer under any other key is NOT read: only DISCOVERY_KEY counts
    fn other_key() -> Value {
        json!({
            "m.homeserver": { "base_url": "https://maplehouse.tinhat.me" },
            "org.example.server": { "server": "0.2.7", "api": "https://maplehouse.tinhat.me/api", "features": { "calls": true, "memories": false, "soundboard": false } }
        })
    }
    fn offer_with(features: Value, extra: Value) -> Value {
        let mut t = json!({ "server": "t", "api": "", "features": features });
        if let (Some(o), Some(e)) = (t.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                o.insert(k.clone(), v.clone());
            }
        }
        json!({ DISCOVERY_KEY: t })
    }
    fn feats(list: &[(ServerFeature, bool)]) -> BTreeMap<ServerFeature, bool> {
        list.iter().copied().collect()
    }
    use ServerFeature::*;

    #[test]
    fn the_key_and_the_feature_list() {
        assert_eq!(DISCOVERY_KEY, "dev.tinhat.server");
        assert_eq!(
            serde_json::to_string(&SERVER_FEATURES.iter().map(|f| f.name()).collect::<Vec<_>>()).unwrap(),
            r#"["calls","soundboard","gifs","watch","games","steam","gameservers","presence","push","memories","reports","guests"]"#
        );
        for f in SERVER_FEATURES {
            assert!(!f.doc().is_empty(), "{} is described", f.name());
            assert_eq!(ServerFeature::parse(f.name()), Some(f));
        }
        assert_eq!(ServerFeature::parse("gif"), None);
    }

    #[test]
    fn parse_the_0_2_21_announcement() {
        let o = parse_offer(&announced());
        assert_eq!(
            o,
            ServerOffer {
                server: "0.2.21".into(),
                api: "https://maplehouse.tinhat.me/api".into(),
                features: feats(&[
                    (Calls, true),
                    (Soundboard, true),
                    (Gifs, true),
                    (Watch, false),
                    (Games, true),
                    (Steam, false),
                    (Gameservers, false),
                    (Presence, true),
                    (Push, true),
                    (Memories, true),
                    (Reports, true),
                    (Guests, true)
                ]),
                recorder: "@recorder:maplehouse.tinhat.me".into(),
                password_reset: PasswordReset::Email,
            }
        );
        for f in [Calls, Soundboard, Gifs, Games, Presence, Push, Memories, Reports, Guests] {
            assert!(feature_on(&o, f), "{}", f.name());
        }
        for f in [Watch, Steam, Gameservers] {
            assert!(!feature_on(&o, f), "{}", f.name());
        }
        // desktop: the API base loses a trailing slash; anything but an http(s) URL is no base (proxy.rs then uses
        // <homeserver origin>/api)
        assert_eq!(parse_offer(&offer_with(json!({}), json!({ "api": "https://x.test/api/" }))).api, "https://x.test/api");
        assert_eq!(parse_offer(&offer_with(json!({}), json!({ "api": "x.test/api" }))).api, "");
    }

    #[test]
    fn anything_else_is_no_offer_another_key_included() {
        for v in [
            Value::Null,
            json!(0),
            json!("x"),
            json!([]),
            json!([announced()]),
            json!({}),
            json!({ "m.homeserver": { "base_url": "https://x" } }),
            other_key(),
            json!({ "dev.tinhat.server": null }),
            json!({ "dev.tinhat.server": [] }),
            json!({ "dev.tinhat.server": "on" }),
        ] {
            assert_eq!(parse_offer(&v), no_offer(), "{v}");
        }
        assert_eq!(no_offer().recorder, "");
    }

    #[test]
    fn only_booleans_count_unknown_keys_and_odd_values_are_ignored() {
        let o = parse_offer(&offer_with(json!({ "memories": "false", "soundboard": 0, "calls": null, "gifs": "no", "extra": true }), json!({ "server": 7, "api": null })));
        assert_eq!(o, ServerOffer { server: "".into(), api: "".into(), features: BTreeMap::new(), recorder: "".into(), password_reset: PasswordReset::Email });
        for f in SERVER_FEATURES {
            assert!(feature_on(&o, f), "{}", f.name());
        }
        let p = parse_offer(&offer_with(json!({ "memories": false }), json!({})));
        assert_eq!(p.features, feats(&[(Memories, false)]));
        assert!(feature_on(&p, Soundboard), "a missing key keeps the feature");
        assert!(!feature_on(&p, Memories));
        // inherited properties never count (a key named __proto__ is just a key)
        let proto: Value = serde_json::from_str(r#"{"dev.tinhat.server": {"features": {"__proto__": {"memories": false}}}}"#).unwrap();
        assert!(feature_on(&parse_offer(&proto), Memories));
    }

    #[test]
    fn the_recorder_a_matrix_id_only_while_memories_is_on() {
        let rec = |features: Value, r: Value| parse_offer(&offer_with(features, json!({ "recorder": r }))).recorder;
        assert_eq!(rec(json!({ "memories": true }), json!("@rec:x.y")), "@rec:x.y");
        assert_eq!(rec(json!({}), json!("@rec:x.y")), "@rec:x.y", "memories absent = on");
        assert_eq!(rec(json!({ "memories": false }), json!("@rec:x.y")), "", "off → no recorder");
        for bad in [json!(""), json!("rec:x.y"), json!("@rec"), json!("@rec:x y"), json!(7), Value::Null, json!(["@rec:x.y"])] {
            assert_eq!(rec(json!({ "memories": true }), bad.clone()), "", "{bad}");
        }
        assert_eq!(parse_offer(&offer_with(json!({ "memories": true }), json!({}))).recorder, "", "none named = none");
        // the MXID rule's edges (/^@[^:\s]+:[^\s]+$/)
        assert_eq!(rec(json!({}), json!("@:x")), "");
        assert_eq!(rec(json!({}), json!("@ab:")), "");
        assert_eq!(rec(json!({}), json!("@a:b:8448")), "@a:b:8448");
        assert_eq!(recorder_of(&parse_offer(&announced())).as_deref(), Some("@recorder:maplehouse.tinhat.me"));
    }

    /// 2026-10-22: a server without email says its owner sets new passwords (password.rs; the web's offer.ts).
    #[test]
    fn the_account_password_reset_owner_or_the_email_form() {
        let pr = |account: Value| parse_offer(&offer_with(json!({}), json!({ "account": account }))).password_reset;
        assert_eq!(pr(json!({ "password_reset": "owner" })), PasswordReset::Owner);
        // missing, another value, not a string, not an object = today's email form
        assert_eq!(parse_offer(&announced()).password_reset, PasswordReset::Email, "0.2.21 says nothing");
        assert_eq!(pr(json!({})), PasswordReset::Email);
        for other in [json!("email"), json!("Owner"), json!(" owner"), json!(""), json!("none")] {
            assert_eq!(pr(json!({ "password_reset": other.clone() })), PasswordReset::Email, "{other}");
        }
        for not_a_string in [json!(true), json!(1), Value::Null, json!(["owner"]), json!({ "owner": true })] {
            assert_eq!(pr(json!({ "password_reset": not_a_string.clone() })), PasswordReset::Email, "{not_a_string}");
        }
        for not_an_object in [json!("owner"), Value::Null, json!(["owner"]), json!(true)] {
            assert_eq!(pr(not_an_object.clone()), PasswordReset::Email, "{not_an_object}");
        }
        // only under dev.tinhat.server
        assert_eq!(parse_offer(&json!({ "account": { "password_reset": "owner" } })).password_reset, PasswordReset::Email);
        // the rest of the offer is untouched by it, and it is a change of the offer
        let o = parse_offer(&json!({ DISCOVERY_KEY: { "server": "0.2.22", "features": { "memories": false }, "account": { "password_reset": "owner" } } }));
        assert_eq!(o.features, feats(&[(Memories, false)]));
        assert!(!same_offer(&o, &ServerOffer { password_reset: PasswordReset::Email, ..o.clone() }));
        // what the store keeps (the document's shape) reads back the same; the email form writes nothing
        let kept = offer_to_json(&o);
        assert_eq!(kept["account"], json!({ "password_reset": "owner" }));
        assert_eq!(parse_offer(&json!({ DISCOVERY_KEY: kept })), o);
        assert!(offer_to_json(&no_offer()).get("account").is_none());
        assert_eq!(no_offer().password_reset, PasswordReset::Email);
    }

    #[test]
    fn feature_on_only_an_explicit_false_turns_a_feature_off() {
        assert!(feature_on(&no_offer(), Memories));
        assert!(feature_on(&ServerOffer { features: feats(&[(Memories, true)]), ..no_offer() }, Memories));
        assert!(!feature_on(&ServerOffer { features: feats(&[(Memories, false)]), ..no_offer() }, Memories));
    }

    #[test]
    fn same_offer_and_predicate_values() {
        assert!(same_offer(&parse_offer(&announced()), &parse_offer(&announced())));
        assert!(!same_offer(&parse_offer(&announced()), &no_offer()));
        assert!(same_offer(&no_offer(), &ServerOffer::default()));
        assert!(
            !same_offer(&ServerOffer { recorder: "@a:b".into(), ..no_offer() }, &ServerOffer { recorder: "@c:d".into(), ..no_offer() }),
            "a new recorder is a change"
        );
        assert_eq!(
            Value::Object(predicate_values(&parse_offer(&announced()), false)),
            json!({
                "server.calls": true, "server.soundboard": true, "server.gifs": true, "server.watch": false,
                "server.games": true, "server.steam": false, "server.gameservers": false, "server.presence": true,
                "server.push": true, "server.memories": true, "server.reports": true, "server.guests": true,
                "selfhost.retired": false
            })
        );
        let all = predicate_values(&no_offer(), false);
        let mut keys: Vec<String> = all.keys().cloned().collect();
        keys.sort();
        let mut want: Vec<String> = SERVER_FEATURES.iter().map(|f| feature_predicate(*f)).chain([RETIRED_PREDICATE.to_string()]).collect();
        want.sort();
        assert_eq!(keys, want);
        assert_eq!(all[RETIRED_PREDICATE], false, "the retired actions are hidden whatever the server offers");
        for f in SERVER_FEATURES {
            assert_eq!(all[&feature_predicate(f)], true, "{}", f.name());
        }
        // a guest account never gets GUEST_WITHHELD (featureFor)
        let g = predicate_values(&no_offer(), true);
        assert_eq!(g["server.reports"], false);
        assert_eq!(g["server.memories"], true);
        assert!(!feature_for(&no_offer(), Reports, true));
        assert!(feature_for(&no_offer(), Reports, false));
        assert!(feature_for(&no_offer(), Guests, true));
    }

    #[test]
    fn is_recorder_id_the_announced_account_as_a_matrix_id_or_a_livekit_identity() {
        let r = "@recorder:maplehouse.tinhat.me";
        assert!(is_recorder_id(r, r));
        assert!(is_recorder_id(&format!("{r}:DEVICE"), r));
        assert!(!is_recorder_id("@recorder:other.example", r));
        assert!(!is_recorder_id("@recorderx:maplehouse.tinhat.me", r));
        assert!(!is_recorder_id("@vod:example.com", r), "another server's recorder account is not this server's recorder");
        assert!(!is_recorder_id(r, ""), "no recorder announced → nothing is filtered");
        assert!(!is_recorder_id("", r));
        assert!(!is_recorder_id("@recorder:maplehouse.tinhat.meX", r));
    }

    #[test]
    fn action_gates_which_actions_each_feature_carries() {
        let cases: &[(&str, Option<ServerFeature>)] = &[
            ("memories.open", Some(Memories)),
            ("memories.sync.toggle", Some(Memories)),
            ("memories.playPause", Some(Memories)),
            ("room.recording.toggle", Some(Memories)),
            ("call.clip", Some(Memories)),
            ("call.share.recordingOnly.toggle", Some(Memories)),
            ("call.recStream.tuneIn", Some(Memories)),
            ("call.recStream.leave", Some(Memories)),
            ("app.recordOptOut.toggle", Some(Memories)),
            ("soundboard.open", Some(Soundboard)),
            ("sound.play", Some(Soundboard)),
            ("sound.upload", Some(Soundboard)),
            ("person.joinSound.play", Some(Soundboard)),
            ("composer.gif", Some(Gifs)),
            ("status.open", Some(Presence)),
            ("status.set", Some(Presence)),
            ("status.clear", Some(Presence)),
            ("presence.ask.answer", Some(Presence)),
            ("presence.sharing.toggle", Some(Presence)),
            ("push.enable", Some(Push)),
            ("push.quiet.toggle", Some(Push)),
            ("push.test", Some(Push)),
            ("app.bugReport", Some(Reports)),
            // not carried by a feature
            ("memoriesX", None),
            ("call.join", None),
            ("call.screenShare.toggle", None),
            ("watchParty.open", None),
            ("watchParty.queue", None),
            ("room.status.post", None), // the room's now-playing status, not Reach
            ("room.status.clear", None),
            ("app.notifications.toggle", None), // in-page notifications need no server
            ("app.notificationPermission", None),
            ("app.diagnostics.create", None),
            ("app.gifAutoplay.toggle", None),
            ("reel.myGames", None),
            ("reel.spin", None),
            ("room.guestLink", None),
            ("call.pushToTalk.toggle", None), // "push." is a prefix of push notifications only
            ("", None),
        ];
        for (id, want) in cases {
            assert_eq!(gate_of_action(id), *want, "{id}");
        }
    }

    #[test]
    fn every_action_gate_names_real_actions_of_the_table() {
        let table: Value = serde_json::from_str(tinhat_actions::table::TABLE_JSON).unwrap();
        let ids: Vec<&str> = table["actions"].as_array().unwrap().iter().map(|a| a["id"].as_str().unwrap()).collect();
        for (k, _) in ACTION_GATES {
            if k.ends_with('.') {
                assert!(ids.iter().any(|id| id.starts_with(k)), "no action starts with {k}");
            } else {
                assert!(ids.contains(k), "no action {k}");
            }
        }
        // a prefix gate never swallows an action of another family
        for id in &ids {
            if id.starts_with("push.") {
                assert_eq!(gate_of_action(id), Some(Push), "{id}");
            }
            if id.starts_with("call.pushToTalk") {
                assert_eq!(gate_of_action(id), None, "{id}");
            }
            if id.starts_with("room.status.") {
                assert_eq!(gate_of_action(id), None, "{id}");
            }
        }
    }

    #[test]
    fn setting_widget_permission_gates_name_real_entries() {
        let settings: Vec<&str> = crate::settings_search::index().entries.iter().map(|e| e.id.as_str()).collect();
        for (k, _) in SETTING_GATES {
            assert!(settings.contains(k), "settings index has no {k}");
        }
        // the rows a guest account never sees (Change password)
        for k in GUEST_HIDDEN_SETTINGS {
            assert!(settings.contains(&k), "settings index has no {k}");
        }
        assert!(!setting_for_guest("security.password", true));
        assert!(setting_for_guest("security.password", false));
        assert!(setting_for_guest("security.devices", true));
        assert_eq!(gate_of_setting("security.password"), None, "every server: it is standard Matrix");
        assert_eq!(gate_of_setting("privacy.never-record"), Some(Memories));
        assert_eq!(gate_of_setting("profile.join-sound"), Some(Soundboard));
        assert_eq!(gate_of_setting("privacy.share-status"), Some(Presence));
        assert_eq!(gate_of_setting("privacy.share-time-zone"), Some(Presence));
        assert_eq!(gate_of_setting("privacy.time-zone"), Some(Presence));
        assert_eq!(gate_of_setting("about.report"), Some(Reports));
        assert_eq!(gate_of_setting("privacy.log-sharing"), Some(Reports));
        assert_eq!(gate_of_setting("guests.state"), Some(Guests));
        assert_eq!(gate_of_setting("guests.link"), Some(Guests));
        assert_eq!(gate_of_setting("privacy.diagnostics"), None);
        assert_eq!(gate_of_setting("notifications.notify-mentions"), None);
        let widgets: Vec<&str> = tinhat_widgets::registry::registry()["widgets"].as_array().unwrap().iter().map(|w| w["id"].as_str().unwrap()).collect();
        for (k, _) in WIDGET_GATES {
            assert!(widgets.contains(k), "widget registry has no {k}");
        }
        assert_eq!(gate_of_widget("memories"), Some(Memories));
        assert_eq!(gate_of_widget("soundPad"), Some(Soundboard));
        assert_eq!(gate_of_widget("gameServers"), Some(Gameservers));
        assert_eq!(gate_of_widget("whosAround"), Some(Presence));
        assert_eq!(gate_of_widget("clocks"), Some(Presence));
        assert_eq!(gate_of_widget("justMe"), Some(Presence));
        for w in ["upNext", "callStats", "pinned", "mentions", "note", "dice", "polls", "gameNight"] {
            assert_eq!(gate_of_widget(w), None, "{w}");
        }
        for (k, _) in PERMISSION_GATES {
            assert!(crate::roles_model::is_known_permission(k), "roles PERMISSIONS has no {k}");
        }
        assert_eq!(gate_of_permission("record_calls"), Some(Memories));
        assert_eq!(gate_of_permission("use_soundboard"), Some(Soundboard));
        assert_eq!(gate_of_permission("manage_soundboard"), Some(Soundboard));
        assert_eq!(gate_of_permission("share_screen"), None);
        assert_eq!(gate_of_permission("manage_games"), None, "the Game Reel itself stays without the catalogue");
        // what QML reads
        let g: Value = serde_json::from_str(&gates_json()).unwrap();
        assert_eq!(g["permissions"]["record_calls"], "memories");
        assert_eq!(g["widgets"]["clocks"], "presence");
        assert_eq!(g["settings"]["about.report"], "reports");
        assert_eq!(g["panes"]["guests"], "guests");
        assert_eq!(g["features"].as_array().unwrap().len(), 12);
        assert_eq!(g["retired"], json!(["room.guestLink", "room.guestLink.disable"]));
        assert_eq!(g["retiredPredicate"], "selfhost.retired");
        assert_eq!(g["guestWithheld"], json!(["reports"]));
    }

    #[test]
    fn apply_action_gates_requires_anded_predicates_declared_idempotent() {
        let mut t = json!({
            "actions": [
                { "id": "memories.open", "when": "room.joined" },
                { "id": "call.clip", "requires": "x.y" },
                { "id": "call.join" },
                { "id": "sound.play", "requires": { "not": "a" } },
                { "id": "composer.gif", "requires": "gif.available" },
                { "id": "push.enable", "requires": "push.available" }
            ],
            "predicates": { "room.joined": {} }
        });
        assert_eq!(apply_action_gates(&mut t), 5);
        assert_eq!(t["actions"][0]["requires"], "server.memories");
        assert_eq!(t["actions"][1]["requires"], json!({ "all": ["x.y", "server.memories"] }));
        assert!(t["actions"][2].get("requires").is_none());
        assert_eq!(t["actions"][3]["requires"], json!({ "all": [{ "not": "a" }, "server.soundboard"] }));
        assert_eq!(t["actions"][4]["requires"], json!({ "all": ["gif.available", "server.gifs"] }));
        assert_eq!(t["actions"][5]["requires"], json!({ "all": ["push.available", "server.push"] }));
        let keys_of = |p: &Value| -> Vec<String> {
            let mut k: Vec<String> = p.as_object().unwrap().keys().cloned().collect();
            k.sort();
            k
        };
        for f in SERVER_FEATURES {
            let p = &t["predicates"][feature_predicate(f)];
            assert_eq!(keys_of(p), ["doc", "scope", "type"]);
            assert_eq!(p["type"], "bool");
            assert_eq!(p["scope"], "global");
            assert_eq!(p["doc"], format!("The server offers {} (a self-hosted Tin Hat Server's announcement; absent = yes).", f.doc()));
        }
        assert_eq!(keys_of(&t["predicates"][RETIRED_PREDICATE]), ["doc", "scope", "type"]);
        assert_eq!(apply_action_gates(&mut t), 0, "a second pass changes nothing");
        // the real table (actions.json): it still loads, and the gated actions carry the rule
        let mut real: Value = serde_json::from_str(tinhat_actions::table::TABLE_JSON).unwrap();
        let n = apply_action_gates(&mut real);
        assert!(n >= 27, "gated {n} actions");
        let loaded = tinhat_actions::table::load(&real).expect("the gated table loads");
        let a = loaded.actions.iter().find(|a| a.id == "memories.open").unwrap();
        assert!(a.requires.as_ref().is_some_and(|r| r.to_string().contains("server.memories")));
        assert!(loaded.predicates.contains_key("server.gifs"));
        assert!(loaded.predicates.contains_key(RETIRED_PREDICATE));
    }

    #[test]
    fn the_guest_room_the_pane_gate_the_retired_per_room_guest_link_guest_accounts() {
        assert_eq!(PANE_GATES, &[("guests", Guests)]);
        assert_eq!(gate_of_pane("guests"), Some(Guests));
        for p in ["invites", "privacy", "about", ""] {
            assert_eq!(gate_of_pane(p), None, "{p}");
        }
        // the per-room guest link is gone whatever the offer
        assert_eq!(RETIRED_ACTIONS, ["room.guestLink", "room.guestLink.disable"]);
        let table: Value = serde_json::from_str(tinhat_actions::table::TABLE_JSON).unwrap();
        let ids: Vec<&str> = table["actions"].as_array().unwrap().iter().map(|a| a["id"].as_str().unwrap()).collect();
        for a in RETIRED_ACTIONS {
            assert!(ids.contains(&a), "no action {a}");
        }
        assert!(is_retired_action("room.guestLink"));
        assert!(is_retired_action("room.guestLink.disable"));
        assert!(!is_retired_action("room.guestLinkX"));
        assert_eq!(gate_of_action("room.guestLink"), None, "retired, not a feature's");
        let mut t = json!({ "actions": [{ "id": "room.guestLink", "requires": "room.canChangeAvatar" }, { "id": "room.guestLink.disable" }], "predicates": {} });
        assert_eq!(apply_action_gates(&mut t), 2);
        assert_eq!(t["actions"][0]["requires"], json!({ "all": ["room.canChangeAvatar", RETIRED_PREDICATE] }));
        assert_eq!(t["actions"][1]["requires"], RETIRED_PREDICATE);
        assert_eq!(predicate_values(&no_offer(), false)[RETIRED_PREDICATE], false);
        // the server's temporary accounts
        assert!(is_guest_account("@guest-k3j2h1abcd:maple.example"));
        for v in ["@guests:maple.example", "@ana:maple.example", "@guest-x", "", "@guest-x:\n"] {
            assert!(!is_guest_account(v), "{v:?}");
        }
    }

    /// Through the REAL engine (the app's actions_ctl face, the overlay laid over the table as at start): a gated
    /// action is hidden (never dimmed) in the run path and the palette while the server says no, and untouched while
    /// it says yes or nothing (offer.test.ts's last case).
    #[test]
    fn through_the_real_engine_hidden_while_the_server_says_no() {
        use cxx_qt_lib::QString;
        assert!(ensure_actions_overlay(), "the overlay must be laid over the table before the engine's first use");
        let room = "!r:x";
        let snap = |server: Map<String, Value>| -> String {
            let mut values = Map::new();
            for (k, v) in [("ui.roomOpen", true), ("call.in", true), ("gif.available", true), ("push.available", true), ("presence.known", true)] {
                values.insert(k.into(), json!(v));
            }
            for (k, v) in server {
                values.insert(k, v);
            }
            json!({
                "values": values,
                "subjects": { "room": { room: { "name": "General", "values": {
                    "room.joined": true, "room.current": true, "room.canChangeAvatar": true, "room.guestLinkOn": true } } } },
                "current": { "room": room }
            })
            .to_string()
        };
        let present = |action: &str, s: &str| -> Value {
            let out = crate::actions_ctl::present(&QString::from(action), &QString::from(s), &QString::from(""), &QString::from("null"), false);
            serde_json::from_str(&out.to_string()).unwrap()
        };
        // the per-room guest link: gone on the desktop, whatever the server offers
        for a in RETIRED_ACTIONS {
            let p = present(a, &snap(predicate_values(&no_offer(), false)));
            assert_eq!(p["visible"], false, "{a} hidden: {p}");
            assert_eq!(p["enabled"], false, "{a} can't run");
        }
        // a server with everything switched off but calls
        let nothing: Map<String, Value> = SERVER_FEATURES.iter().map(|f| (f.name().to_string(), json!(*f == Calls))).collect();
        let off = snap(predicate_values(&parse_offer(&offer_with(Value::Object(nothing), json!({}))), false));
        let on = snap(predicate_values(&no_offer(), false));
        for a in [
            "memories.open",
            "room.recording.toggle",
            "soundboard.open",
            "app.recordOptOut.toggle",
            "composer.gif",
            "status.open",
            "status.set",
            "presence.sharing.toggle",
            "app.bugReport",
        ] {
            let p = present(a, &off);
            assert_eq!(p["visible"], false, "{a} hidden: {p}");
            assert_eq!(p["enabled"], false, "{a} can't run");
            assert_eq!(p["why"], "requires");
            assert_eq!(present(a, &on)["visible"], true, "{a} shown when the server offers it");
        }
        // push.* are web-only actions (the desktop has no push): hidden either way, the gate changes nothing here
        for a in ["push.enable", "push.test"] {
            assert_eq!(present(a, &off)["visible"], false, "{a}");
            assert_eq!(present(a, &on)["visible"], false, "{a} (web only)");
        }
        // one feature at a time: only its own actions go
        let only = |f: &str| snap(predicate_values(&parse_offer(&offer_with(json!({ f: false }), json!({}))), false));
        assert_eq!(present("composer.gif", &only("gifs"))["visible"], false);
        assert_eq!(present("app.bugReport", &only("gifs"))["visible"], true);
        assert_eq!(present("soundboard.open", &only("presence"))["visible"], true); // (the web checks push.enable: web only)
        assert_eq!(present("status.open", &only("presence"))["visible"], false);
        // a guest account: no Report a problem, the rest as offered
        let guest = snap(predicate_values(&no_offer(), true));
        assert_eq!(present("app.bugReport", &guest)["visible"], false);
        assert_eq!(present("memories.open", &guest)["visible"], true);
        // an action no feature carries is untouched
        for a in ["call.deafen.toggle", "watchParty.open", "room.status.post", "app.notifications.toggle", "reel.myGames"] {
            assert_eq!(present(a, &off)["visible"], true, "{a}");
        }
        // Ctrl+K: "Memories", "Soundboard", "status", "Report a problem" are not offered while the server says no
        let palette = |s: &str, q: &str| -> String { crate::actions_ctl::palette(&QString::from(s), &QString::from(q), &QString::from("")).to_string() };
        for (q, a) in [("memories", "memories.open"), ("soundboard", "soundboard.open"), ("status", "status.open"), ("report", "app.bugReport")] {
            assert!(!palette(&off, q).contains(&format!("\"{a}\"")), "{a} not in the palette");
            assert!(palette(&on, q).contains(&format!("\"{a}\"")), "{a} in the palette when offered");
        }
    }

    /// Astra review 2026-10-21: a well-known can't send the session's bearer token to a third host.
    #[test]
    fn the_announced_api_must_be_this_servers_host_over_https() {
        let hs = "https://maplehouse.tinhat.me/";
        assert!(api_allowed("https://maplehouse.tinhat.me/api", hs, "maplehouse.tinhat.me"));
        assert!(api_allowed("https://maplehouse.tinhat.me:8448/api", hs, "maplehouse.tinhat.me"));
        // delegation: the server name's host or the homeserver's
        assert!(api_allowed("https://chat.example.com/api", "https://chat.example.com", "example.com"));
        assert!(api_allowed("https://example.com/api", "https://chat.example.com", "example.com"));
        // a third host, user-info tricks, a query, plain http on an https server, junk
        assert!(!api_allowed("https://evil.example/api", hs, "maplehouse.tinhat.me"));
        assert!(!api_allowed("https://maplehouse.tinhat.me@evil.example/api", hs, "maplehouse.tinhat.me"));
        assert!(!api_allowed("https://maplehouse.tinhat.me.evil.example/api", hs, "maplehouse.tinhat.me"));
        assert!(!api_allowed("https://maplehouse.tinhat.me/api?x=1", hs, "maplehouse.tinhat.me"));
        assert!(!api_allowed("http://maplehouse.tinhat.me/api", hs, "maplehouse.tinhat.me"));
        assert!(!api_allowed("maplehouse.tinhat.me/api", hs, "maplehouse.tinhat.me"));
        assert!(!api_allowed("", hs, "maplehouse.tinhat.me"));
        // a plain-http test server may name its own plain-http API
        assert!(api_allowed("http://localhost:8008/api", "http://localhost:8008", "localhost"));
        assert!(!api_allowed("http://other:8008/api", "http://localhost:8008", "localhost"));
    }

    #[test]
    fn the_store_keeps_an_answer_and_forgets_it() {
        // the store is process-wide; this test owns it for its duration (other tests here don't touch it)
        let o = parse_offer(&json!({ "dev.tinhat.server": {
            "server": "0.2.21", "api": "https://maplehouse.tinhat.me/api",
            "features": { "memories": false, "reports": true, "guests": false } } }));
        set_for_test(o.clone());
        assert!(!feature_on_global(Memories));
        assert!(feature_on_global(Reports));
        assert!(!feature_on_global(Guests));
        assert_eq!(recorder(), None);
        assert_eq!(api_base().as_deref(), Some("https://maplehouse.tinhat.me/api"));
        let j: Value = serde_json::from_str(&offer_json()).unwrap();
        assert_eq!(j["features"]["memories"], false);
        assert_eq!(j["api"], "https://maplehouse.tinhat.me/api");
        assert_eq!(j["guest"], false, "nobody signed in in the test binary");
        let r1 = rev();
        set_for_test(o.clone());
        assert_eq!(rev(), r1, "the same offer is no change");
        set_for_test(parse_offer(&json!({ "dev.tinhat.server": { "features": { "memories": true }, "recorder": "@recorder:maplehouse.tinhat.me",
                                                                 "account": { "password_reset": "owner" } } })));
        assert_eq!(recorder().as_deref(), Some("@recorder:maplehouse.tinhat.me"));
        assert_eq!(current().password_reset, PasswordReset::Owner);
        assert_eq!(serde_json::from_str::<Value>(&offer_json()).unwrap()["account"]["password_reset"], "owner");
        assert!(rev() > r1);
        assert_eq!(pinned_offer(), None, "TINHAT_SERVER_OFFER is not set in the test binary");
        let e1 = lock().epoch;
        lock().in_flight = true; // a read on its way at sign-out …
        forget();
        assert!(lock().epoch > e1, "… belongs to the account that left (refresh drops it)");
        assert!(!lock().in_flight, "… and doesn't hold up the next account's first read");
        assert!(former_names().is_empty());
        assert_eq!(current(), no_offer());
        assert_eq!(current().password_reset, PasswordReset::Email, "sign-out forgets it");
        assert_eq!(recorder(), None);
        assert_eq!(api_base(), None);
        assert!(feature_on_global(Memories));
    }
}
