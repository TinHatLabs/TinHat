//! callrules.rs — the pure call rules of the calls round (security review M1 + M2), the same rules as Tin Hat Web's
//! `src/features/calls/callrules.ts` and the mobile core's `src/callrules.rs` (one product: the same verdict for the
//! same input). Contract: share `claude/tinhat-security-review/fixes/calls-round/DESIGN.md` §2–§4. The shared vectors
//! `testvectors/calls.json` are owned here; web and core copy them byte for byte.
//!
//! - M1, who hosts the call ([`focus_decision`]): the oldest live membership's call service, as in 12.0.x, now judged
//!   before any token leaves: `own` (our server's service), `trusted` (a host the user chose "Always trust" for),
//!   `ask` (any other https host: the sheet), `refuse` (http, user-info, `?`/`#`, a bad address, or a LiveKit room
//!   name that isn't this room). A refused / declined running call is never replaced by one on our own service.
//! - The call service's answer ([`sfu_url_ok`]): only a `wss:` / `https:` LiveKit address is connected to.
//! - M2, who gets our media key ([`key_recipients_v2`]): the web's rule (live, well-formed, joined, not us) plus
//!   "the device still exists" and "it hasn't left the call server in this session".
//! - M2, rotate on leave ([`Planner`]): a holder of our current / upcoming key leaves the recipient set → after
//!   [`DEBOUNCE_MS`] mint index+1 for the rest, use it [`USE_DELAY_MS`] later; at most one key pending.
//! - The send boundary ([`dispatch_checked`]): every key send re-checks each device right before its own dispatch.
//!
//! No app or SDK types here (`serde_json::Value` in, plain structs out) so the mobile core can port it verbatim.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::keyrules::{self, LiveSession};

/// Batch leaves and absorb flaps before a new key is minted.
pub const DEBOUNCE_MS: u64 = 3000;
/// Start encrypting with a minted key this long after it was sent (receivers install it first).
pub const USE_DELAY_MS: u64 = 3000;
/// Key indices live in `0..RING` (libwebrtc's 255-slot ring; index 255 is never used).
pub const RING: u16 = 255;

// ---------------------------------------------------------------------------------------------------------------
// Addresses (DESIGN §2.3)

fn has_control(s: &str) -> bool {
    s.chars().any(|c| (c as u32) <= 0x20 || c as u32 == 0x7f)
}

/// The canonical form of a call service (`livekit_service_url`): `https://` + host (as the WHATWG parser gives it:
/// lower case, punycode) + `:port` unless 443 + the path without trailing `/`. Errors: `"insecure-scheme"` (http),
/// `"bad-url"` (anything else wrong). `?` / `#` and control characters are refused BEFORE parsing (WHATWG and the
/// `url` crate disagree on an empty query; WHATWG strips tabs and newlines).
pub fn normalize_focus_url(s: &str) -> Result<String, &'static str> {
    if s.is_empty() || s.encode_utf16().count() > 2048 || has_control(s) || s.contains('?') || s.contains('#') {
        return Err("bad-url");
    }
    let u = url::Url::parse(s).map_err(|_| "bad-url")?;
    match u.scheme() {
        "https" => {}
        "http" => return Err("insecure-scheme"),
        _ => return Err("bad-url"),
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err("bad-url");
    }
    let host = u.host_str().filter(|h| !h.is_empty()).ok_or("bad-url")?;
    let port = u.port().map(|p| format!(":{p}")).unwrap_or_default(); // None for the default 443
    Ok(format!("https://{host}{port}{}", u.path().trim_end_matches('/')))
}

/// `host[:port]` of a normalised call service: what the sheet names and the allow-list holds.
pub fn host_of(normalised: &str) -> String {
    let rest = normalised.strip_prefix("https://").unwrap_or(normalised);
    rest.split('/').next().unwrap_or("").to_string()
}

/// The LiveKit address a call service answered with: no control characters, parses, `wss` or `https`, a host, no
/// user-info.
pub fn sfu_url_ok(s: &str) -> bool {
    if s.is_empty() || has_control(s) {
        return false;
    }
    let Ok(u) = url::Url::parse(s) else { return false };
    matches!(u.scheme(), "wss" | "https")
        && u.host_str().map(|h| !h.is_empty()).unwrap_or(false)
        && u.username().is_empty()
        && u.password().is_none()
}

// ---------------------------------------------------------------------------------------------------------------
// M1: the call host policy (DESIGN §2.1–§2.2)

/// The first `{type: livekit, livekit_service_url}` of a `.well-known/matrix/client`'s `org.matrix.msc4143.rtc_foci`.
fn wellknown_focus(wk: &serde_json::Value) -> Option<String> {
    wk.get("org.matrix.msc4143.rtc_foci")?.as_array()?.iter().find_map(|f| {
        if f.get("type").and_then(|t| t.as_str()) != Some("livekit") {
            return None;
        }
        f.get("livekit_service_url").and_then(|u| u.as_str()).filter(|u| !u.is_empty()).map(str::to_string)
    })
}

/// Our own call services: `(start, set)`. `set` = {the well-known's livekit service if it normalises (https), the
/// homeserver's `<origin>/livekit/jwt` if it does}; `start` = the first of those (where a new call begins), "" when
/// neither is a valid https address — then [`focus_decision`] refuses to start (never an http start).
pub fn own_foci(wk: &serde_json::Value, hs_origin: &str) -> (String, Vec<String>) {
    let fallback_raw = format!("{}/livekit/jwt", hs_origin.trim_end_matches('/'));
    let fallback = normalize_focus_url(&fallback_raw).ok();
    let wk = wellknown_focus(wk).and_then(|u| normalize_focus_url(&u).ok());
    let mut set: Vec<String> = Vec::new();
    for u in [wk.clone(), fallback.clone()].into_iter().flatten() {
        if !set.contains(&u) {
            set.push(u);
        }
    }
    let start = wk.or(fallback).unwrap_or_default();
    (start, set)
}

/// What a join does (DESIGN §2.2). Fields that don't apply to a decision are `None` (the vectors require them
/// absent): `host` only for trusted / ask, `reason` only for refuse, `service_url` / `alias` for all but refuse,
/// `advertiser` whenever a call is running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FocusDecision {
    /// "own" | "trusted" | "ask" | "refuse"
    pub decision: &'static str,
    /// a call is running (false = we start one on `start`)
    pub call: bool,
    pub service_url: Option<String>,
    pub alias: Option<String>,
    pub host: Option<String>,
    /// the sender of the membership that picked the service ("started by" on the sheet)
    pub advertiser: Option<String>,
    /// refuse only: "bad-url" | "insecure-scheme" | "alias-mismatch"
    pub reason: Option<&'static str>,
}

impl FocusDecision {
    /// own / trusted: join without asking.
    pub fn may_join(&self) -> bool {
        matches!(self.decision, "own" | "trusted")
    }
    /// The vectors' `expect` shape (absent keys left out).
    pub fn to_json(&self) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        m.insert("decision".into(), self.decision.into());
        m.insert("call".into(), self.call.into());
        let mut put = |k: &str, v: &Option<String>| {
            if let Some(v) = v {
                m.insert(k.into(), v.clone().into());
            }
        };
        put("serviceUrl", &self.service_url);
        put("alias", &self.alias);
        put("host", &self.host);
        put("advertiser", &self.advertiser);
        if let Some(r) = self.reason {
            m.insert("reason".into(), r.into());
        }
        serde_json::Value::Object(m)
    }
}

/// The live session whose service a join uses: the oldest by (`created_ts`, sender, device id), compared as bytes —
/// a TOTAL order, so every client picks the same one even when `created_ts` ties. Sessions without a livekit focus
/// don't count. Room membership is deliberately NOT checked (12.0.x doesn't either; checking would split calls).
pub fn running_session(sessions: &[LiveSession]) -> Option<&LiveSession> {
    sessions
        .iter()
        .filter(|s| s.focus.is_some())
        .min_by(|a, b| (a.created_ts, a.sender.as_bytes(), a.device_id.as_bytes()).cmp(&(b.created_ts, b.sender.as_bytes(), b.device_id.as_bytes())))
}

/// DESIGN §2.2 over raw `org.matrix.msc3401.call.member` state events (`{sender, state_key, origin_server_ts,
/// content}`). `own_set` / `start` from [`own_foci`]; `allow` = account data `dev.tinhat.call_hosts` hosts.
pub fn focus_decision(room: &str, now: u64, events: &[serde_json::Value], own_set: &[String], start: &str, allow: &[String]) -> FocusDecision {
    let sessions = keyrules::live_sessions_from(events, now);
    let Some(s) = running_session(&sessions) else {
        let start = match normalize_focus_url(start) {
            Ok(n) => n,
            Err(reason) => return FocusDecision { decision: "refuse", call: false, service_url: None, alias: None, host: None, advertiser: None, reason: Some(reason) },
        };
        return FocusDecision {
            decision: "own",
            call: false,
            service_url: Some(start),
            alias: Some(room.to_string()),
            host: None,
            advertiser: None,
            reason: None,
        };
    };
    let (url, alias) = s.focus.clone().unwrap_or_default();
    let advertiser = Some(s.sender.clone());
    let refuse = |reason: &'static str| FocusDecision {
        decision: "refuse",
        call: true,
        service_url: None,
        alias: None,
        host: None,
        advertiser: advertiser.clone(),
        reason: Some(reason),
    };
    let norm = match normalize_focus_url(&url) {
        Ok(n) => n,
        Err(r) => return refuse(r),
    };
    if !alias.is_empty() && alias != room {
        return refuse("alias-mismatch");
    }
    let host = host_of(&norm);
    let (decision, host) = if own_set.iter().any(|o| *o == norm) {
        ("own", None)
    } else if allow.iter().any(|a| a.to_ascii_lowercase() == host) {
        ("trusted", Some(host))
    } else {
        ("ask", Some(host))
    };
    FocusDecision { decision, call: true, service_url: Some(norm), alias: Some(room.to_string()), host, advertiser, reason: None }
}

/// The hosts of account data `dev.tinhat.call_hosts` (`{"v":1,"allow":[…]}`); anything else = none.
pub fn allow_list_from(content: &serde_json::Value) -> Vec<String> {
    content
        .get("allow")
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(|h| h.as_str()).filter(|h| !h.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

/// `content` with `host` added to (`add`) or removed from the allow-list, the other fields kept.
pub fn allow_list_with(content: &serde_json::Value, host: &str, add: bool) -> serde_json::Value {
    let mut hosts = allow_list_from(content);
    hosts.retain(|h| h != host);
    if add && !host.is_empty() {
        hosts.push(host.to_string());
    }
    let mut out = content.as_object().cloned().unwrap_or_default();
    out.insert("v".into(), 1.into());
    out.insert("allow".into(), hosts.into());
    serde_json::Value::Object(out)
}

/// The words of the "hosted elsewhere" sheet (DESIGN §2.4, the same on web, desktop and phone): `{title, body, join,
/// always, cancel}`. An empty `advertiser_name` shows the id in its place (the web: `advertiserName || advertiser`).
pub fn host_sheet_words(host: &str, me: &str, advertiser_name: &str, advertiser: &str) -> serde_json::Value {
    let by = format!("{} ({advertiser})", if advertiser_name.is_empty() { advertiser } else { advertiser_name });
    serde_json::json!({
        "title": "Join a call hosted elsewhere?",
        "body": format!("This call runs on {host}, not on your server. Joining gives {host} your IP address and a sign-in proof that says you are {me} (valid for up to an hour). What you say and show stays end-to-end encrypted. Started by {by}."),
        "join": "Join",
        "always": format!("Always trust {host}"),
        "cancel": "Cancel",
    })
}

/// What the call bar says when a join is refused (DESIGN §2.4).
pub fn refusal_text(reason: &str) -> &'static str {
    match reason {
        "insecure-scheme" => "This call runs on an unencrypted address, so Tin Hat won't join it.",
        "alias-mismatch" => "This call points at a different room, so Tin Hat won't join it.",
        "foreign-host" => "This call runs on another server, so Tin Hat won't join it.",
        _ => "This call's address isn't valid, so Tin Hat won't join it.",
    }
}

/// What the call bar says when the call service answers with an address we won't connect to (DESIGN §2.3).
pub const UNSAFE_SFU_TEXT: &str = "The call service answered with an unsafe address";

/// Q1 (the maintainer's call, DESIGN §6): calls hosted by another server are asked about (true) or refused outright (false).
/// Flip to false to refuse every foreign call service; trusted hosts then don't count either.
pub const ASK_FOR_FOREIGN_HOSTS: bool = true;

/// [`focus_decision`] under the Q1 switch: with [`ASK_FOR_FOREIGN_HOSTS`] off, `ask` and `trusted` become a refusal.
pub fn apply_foreign_policy(d: FocusDecision) -> FocusDecision {
    if ASK_FOR_FOREIGN_HOSTS || !matches!(d.decision, "ask" | "trusted") {
        return d;
    }
    FocusDecision { decision: "refuse", service_url: None, alias: None, reason: Some("foreign-host"), ..d }
}

// ---------------------------------------------------------------------------------------------------------------
// M2: recipients v2 (DESIGN §3.1)

/// Split a LiveKit identity `@user:server:DEVICE` at the LAST `:` (the web's `util.ts`). `None` for identities that
/// aren't `@u:s` + device (hashed MSC4143 identities): those are ignored by the call-server clause.
pub fn split_identity(identity: &str) -> Option<(String, String)> {
    let (user, device) = identity.rsplit_once(':')?;
    if !user.starts_with('@') || !user[1..].contains(':') || user.len() < 4 || device.is_empty() {
        return None;
    }
    Some((user.to_string(), device.to_string()))
}

/// Who may receive our media key, as sorted `user|device` ids: the web's `keyRecipients` (live, well-formed, joined —
/// `None` = unknown = not yet — not our own device) AND the device is in its user's device list when we know that
/// list (`devices`; an absent user excludes no one) AND there is no `sfu_gone` entry `(user:device, created_ts)` for
/// this session (a later `created_ts` is a new session = a join).
pub fn key_recipients_v2(
    sessions: &[LiveSession],
    membership_of: &dyn Fn(&str) -> Option<String>,
    me_user: &str,
    me_device: &str,
    devices: &HashMap<String, Vec<String>>,
    sfu_gone: &[(String, u64)],
) -> Vec<String> {
    let created: HashMap<(&str, &str), u64> = {
        let mut m = HashMap::new();
        for s in sessions {
            m.entry((s.sender.as_str(), s.device_id.as_str())).or_insert(s.created_ts);
        }
        m
    };
    let gone: HashSet<(&str, u64)> = sfu_gone.iter().map(|(id, ts)| (id.as_str(), *ts)).collect();
    let mut out: Vec<String> = keyrules::key_recipients(sessions, membership_of, me_user, me_device)
        .into_iter()
        .filter(|(u, d)| devices.get(u).map(|l| l.iter().any(|x| x == d)).unwrap_or(true))
        .filter(|(u, d)| {
            let ts = created.get(&(u.as_str(), d.as_str())).copied().unwrap_or(0);
            !gone.contains(&(format!("{u}:{d}").as_str(), ts))
        })
        .map(|(u, d)| format!("{u}|{d}"))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The book of devices that left the call server in this call (DESIGN §3.1), kept only while OUR room is connected.
#[derive(Clone, Debug, Default)]
pub struct SfuGone {
    entries: Vec<(String, u64)>,
}

impl SfuGone {
    /// `ParticipantDisconnected(identity)`: recorded with the live session's `created_ts` when there is one.
    pub fn left(&mut self, identity: &str, sessions: &[LiveSession]) {
        let Some((u, d)) = split_identity(identity) else { return };
        let Some(s) = sessions.iter().find(|s| s.sender == u && s.device_id == d) else { return };
        let id = format!("{u}:{d}");
        self.entries.retain(|(i, _)| *i != id);
        self.entries.push((id, s.created_ts));
    }
    /// `ParticipantConnected(identity)`.
    pub fn joined(&mut self, identity: &str) {
        if let Some((u, d)) = split_identity(identity) {
            let id = format!("{u}:{d}");
            self.entries.retain(|(i, _)| *i != id);
        }
    }
    /// Our own connect / reconnect / rejoin.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
    pub fn entries(&self) -> &[(String, u64)] {
        &self.entries
    }
}

// ---------------------------------------------------------------------------------------------------------------
// M2: the rotate-on-leave planner (DESIGN §3.3)

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// send each key (current first) as its own event to `to`
    Announce { keys: Vec<u8>, to: Vec<String> },
    /// mint a new key at `index` and send it to `to`
    Mint { index: u8, to: Vec<String> },
    /// start encrypting with this index
    Use(u8),
}

impl Action {
    /// The vectors' action shape.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Action::Announce { keys, to } => serde_json::json!({"announce": {"keys": keys, "to": to}}),
            Action::Mint { index, to } => serde_json::json!({"mint": {"index": index, "to": to}}),
            Action::Use(i) => serde_json::json!({"use": i}),
        }
    }
}

#[derive(Clone, Debug)]
struct Pending {
    index: u8,
    use_at: u64,
    holders: BTreeSet<String>,
}

/// The rotate-on-leave state of one call (pure: time and the recipient set R come in, actions go out).
#[derive(Clone, Debug)]
pub struct Planner {
    cur: u8,
    cur_holders: BTreeSet<String>,
    pending: Option<Pending>,
    depart_at: Option<u64>,
    rotate: bool,
}

fn sorted(set: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut v: Vec<String> = set.into_iter().collect();
    v.sort();
    v.dedup();
    v
}

impl Planner {
    /// A new call: `index` in use, announced to R.
    pub fn start(index: u8, _now: u64, r: &[String]) -> (Planner, Vec<Action>) {
        let p = Planner { cur: index, cur_holders: r.iter().cloned().collect(), pending: None, depart_at: None, rotate: true };
        let out = if r.is_empty() { vec![] } else { vec![Action::Announce { keys: vec![index], to: sorted(r.iter().cloned()) }] };
        (p, out)
    }

    /// The server's kill switch (`dev.tinhat.server.features.call_key_rotation == false`): keep tracking, mint nothing.
    pub fn set_rotation(&mut self, on: bool) {
        self.rotate = on;
    }

    /// The index we encrypt with.
    pub fn in_use(&self) -> u8 {
        self.cur
    }

    /// The minted, not yet used index.
    pub fn pending(&self) -> Option<u8> {
        self.pending.as_ref().map(|p| p.index)
    }

    /// What every ladder / steady / refresh / resume announce sends: current, then pending.
    pub fn announce_keys(&self) -> Vec<u8> {
        let mut v = vec![self.cur];
        if let Some(p) = &self.pending {
            v.push(p.index);
        }
        v
    }

    /// A key goes to `ids` outside [`Planner::step`] (join, ladder, steady, refresh announces): they hold it from now
    /// on, so their departure rotates it. Call right BEFORE the send (a send that may have gone out counts).
    /// = the web's `noteHolders`.
    pub fn note_holders(&mut self, ids: &[String], indices: &[u8]) {
        if indices.contains(&self.cur) {
            self.cur_holders.extend(ids.iter().cloned());
        }
        if let Some(p) = self.pending.as_mut().filter(|p| indices.contains(&p.index)) {
            p.holders.extend(ids.iter().cloned());
        }
    }

    fn switch_if_due(&mut self, now: u64, out: &mut Vec<Action>) {
        if self.pending.as_ref().map(|p| now >= p.use_at).unwrap_or(false) {
            let p = self.pending.take().unwrap();
            self.cur = p.index;
            self.cur_holders = p.holders;
            out.push(Action::Use(self.cur));
        }
    }

    /// Only step 1 of [`Planner::step`]: a pending key whose use time has come becomes the key in use (`[Use]` or
    /// `[]`). Driven by its own timer, so a slow recipient read never delays the switch away from a leaver's key
    /// (= the web's `tick`).
    pub fn tick(&mut self, now: u64) -> Vec<Action> {
        let mut out = Vec::new();
        self.switch_if_due(now, &mut out);
        out
    }

    /// When the pending key goes into use (None = nothing pending).
    pub fn use_at(&self) -> Option<u64> {
        self.pending.as_ref().map(|p| p.use_at)
    }

    /// Re-evaluate with the current recipient set R (sorted or not) at `now`.
    pub fn step(&mut self, now: u64, r: &[String]) -> Vec<Action> {
        let mut out = Vec::new();
        let r_set: BTreeSet<String> = r.iter().cloned().collect();
        // 1. a pending key whose time has come
        self.switch_if_due(now, &mut out);
        // 2. newcomers get every key they'll need
        let base = self.pending.as_ref().map(|p| &p.holders).unwrap_or(&self.cur_holders);
        let newcomers: Vec<String> = r_set.difference(base).cloned().collect();
        if !newcomers.is_empty() {
            out.push(Action::Announce { keys: self.announce_keys(), to: newcomers });
        }
        // 3. everyone in R now holds what we announce
        self.cur_holders.extend(r_set.iter().cloned());
        if let Some(p) = self.pending.as_mut() {
            p.holders.extend(r_set.iter().cloned());
        }
        // 4. who holds the upcoming key and left?
        let holders = self.pending.as_ref().map(|p| &p.holders).unwrap_or(&self.cur_holders);
        if holders.difference(&r_set).next().is_none() {
            self.depart_at = None;
        } else if self.depart_at.is_none() {
            self.depart_at = Some(now);
        }
        // 5. debounced: mint the next key for R
        if let Some(t) = self.depart_at {
            if self.pending.is_none() && now.saturating_sub(t) >= DEBOUNCE_MS && self.rotate {
                let index = ((self.cur as u16 + 1) % RING) as u8;
                let use_at = if r_set.is_empty() { now } else { now + USE_DELAY_MS };
                self.pending = Some(Pending { index, use_at, holders: r_set.clone() });
                self.depart_at = None;
                out.push(Action::Mint { index, to: r_set.iter().cloned().collect() });
            }
        }
        // 6. (an empty R mints and uses in the same step)
        self.switch_if_due(now, &mut out);
        out
    }

    /// When the driver should look again: 1 s while a departure or a pending key exists, else 5 s (the backstop).
    pub fn next_tick_ms(&self) -> u64 {
        if self.depart_at.is_some() || self.pending.is_some() {
            1000
        } else {
            5000
        }
    }
}

/// Is index `a` newer than `b` on the ring (0 follows 254; distance 1..=127 forward = newer)? The web's `newerIndex`
/// is `(a - b + 255) % 255 < 128`, i.e. this or equal.
pub fn newer_index(a: u8, b: u8) -> bool {
    let d = (a as i32 - b as i32).rem_euclid(RING as i32);
    d != 0 && d < 128
}

/// Resend obligations (Astra, web round 3/4): per device, the NEWEST key index it still has to get. A failed send of
/// index i owes at least i; a delivery of index i clears the obligation only when it is i or newer — a late success of
/// an older key never cancels a newer one.
#[derive(Clone, Debug, Default)]
pub struct ResendBook {
    owed: std::collections::BTreeMap<String, u8>,
}

impl ResendBook {
    pub fn failed(&mut self, id: &str, index: u8) {
        match self.owed.get(id) {
            Some(cur) if !newer_index(index, *cur) => {}
            _ => {
                self.owed.insert(id.to_string(), index);
            }
        }
    }
    pub fn delivered(&mut self, id: &str, index: u8) {
        if let Some(owed) = self.owed.get(id) {
            if index == *owed || newer_index(index, *owed) {
                self.owed.remove(id);
            }
        }
    }
    /// Drop obligations for keys no longer announced (superseded: a newer mint owes its own set).
    pub fn keep_only(&mut self, live: &[u8]) {
        self.owed.retain(|_, i| live.contains(i));
    }
    /// The devices still owed something, sorted.
    pub fn to(&self) -> Vec<String> {
        self.owed.keys().cloned().collect()
    }
    pub fn is_empty(&self) -> bool {
        self.owed.is_empty()
    }
}

/// After a switch, the recipients whose key send failed get the keys again at these delays after `use` (the
/// coordinator's rule for both apps; the 60 s steady announce is the backstop after the last rung).
pub const RESEND_AFTER_USE_MS: [u64; 3] = [2000, 5000, 10000];

/// The resend ladder for `failed` (`user|device` ids) after a switch at `used_at`: one rung per delay, sorted ids
/// (= the web's `resendPlan`).
pub fn resend_plan(failed: &[String], used_at: u64) -> Vec<(u64, Vec<String>)> {
    let to = sorted(failed.iter().cloned());
    if to.is_empty() {
        return Vec::new();
    }
    RESEND_AFTER_USE_MS.iter().map(|d| (used_at + d, to.clone())).collect()
}

// ---------------------------------------------------------------------------------------------------------------
// The send boundary (12.0.5 review item 1)

/// What [`dispatch_checked`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DispatchReport {
    pub sent: Vec<String>,
    /// no longer eligible at their own dispatch
    pub dropped: Vec<String>,
    pub failed: Vec<String>,
}

/// Send to each target as its own batch, re-checking that target's eligibility right before its dispatch, so a
/// leave / kick that lands while earlier batches (and their retries) are in flight drops the later ones. `still_ok`
/// should read the local store only (no network). The window that remains: the SDK's one-time-key claim and the HTTP
/// request of that one batch, after its check.
pub async fn dispatch_checked<T, C, CF, S, SF>(targets: Vec<T>, id_of: impl Fn(&T) -> String, mut still_ok: C, mut send: S) -> DispatchReport
where
    C: FnMut(String) -> CF,
    CF: std::future::Future<Output = bool>,
    S: FnMut(T) -> SF,
    SF: std::future::Future<Output = bool>,
{
    let mut rep = DispatchReport::default();
    for t in targets {
        let id = id_of(&t);
        if !still_ok(id.clone()).await {
            rep.dropped.push(id);
            continue;
        }
        if send(t).await {
            rep.sent.push(id);
        } else {
            rep.failed.push(id);
        }
    }
    rep
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    /// The shared vectors (the owner's copy; web and the mobile core hold byte copies).
    pub(crate) const VECTORS: &str = include_str!("testvectors/calls.json");

    fn vectors() -> Value {
        serde_json::from_str(VECTORS).unwrap()
    }

    fn merge_patch(target: &mut Value, patch: &Value) {
        let (Some(t), Some(p)) = (target.as_object_mut(), patch.as_object()) else { return };
        for (k, v) in p {
            if v.is_null() {
                t.remove(k);
            } else if v.is_object() && t.get(k).map(|x| x.is_object()).unwrap_or(false) {
                merge_patch(t.get_mut(k).unwrap(), v);
            } else {
                t.insert(k.clone(), v.clone());
            }
        }
    }

    /// The vectors' member shorthand `{s, d, c, f?, a?, sk?, patch?}` → a raw membership event (`memberTemplate`).
    fn expand_member(m: &Value, room: &str) -> Value {
        let g = |k: &str| m.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let (s, d) = (g("s").unwrap(), g("d").unwrap());
        let c = m["c"].as_u64().unwrap();
        let mut content = json!({
            "application": "m.call", "call_id": "", "scope": "m.room", "device_id": d,
            "focus_active": { "type": "livekit", "focus_selection": "oldest_membership" },
            "foci_preferred": [{ "type": "livekit",
                "livekit_service_url": g("f").unwrap_or_else(|| "https://a.test/livekit/jwt".into()),
                "livekit_alias": g("a").unwrap_or_else(|| room.to_string()) }],
            "created_ts": c, "expires": 14_400_000u64, "membershipID": format!("{s}:{d}"),
        });
        if let Some(p) = m.get("patch") {
            merge_patch(&mut content, p);
        }
        json!({ "type": "org.matrix.msc3401.call.member", "sender": s,
                "state_key": g("sk").unwrap_or_else(|| format!("_{s}_{d}_m.call")), "origin_server_ts": c, "content": content })
    }

    fn field<'a>(case: &'a Value, defs: &'a Value, k: &str) -> &'a Value {
        case.get(k).unwrap_or(&defs[k])
    }

    fn strs(v: &Value) -> Vec<String> {
        v.as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn calls_vectors_defaults_match_the_constants() {
        let v = vectors();
        assert_eq!(v["v"], 1);
        let d = &v["defaults"];
        assert_eq!(d["debounceMs"], DEBOUNCE_MS);
        assert_eq!(d["useDelayMs"], USE_DELAY_MS);
        assert_eq!(d["ring"], RING);
        assert_eq!(d["expiresMs"], keyrules::MEMBERSHIP_EXPIRES_MS);
    }

    #[test]
    fn calls_vectors_focus() {
        let v = vectors();
        let defs = &v["defaults"];
        let cases = v["focus"].as_array().unwrap();
        assert_eq!(cases.len(), 17);
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let room = field(case, defs, "room").as_str().unwrap();
            let now = field(case, defs, "now").as_u64().unwrap();
            let hs = field(case, defs, "hs").as_str().unwrap();
            let (start, set) = own_foci(field(case, defs, "wellKnown"), hs);
            let allow = strs(field(case, defs, "allow"));
            let events: Vec<Value> = case["members"].as_array().unwrap().iter().map(|m| expand_member(m, room)).collect();
            let got = focus_decision(room, now, &events, &set, &start, &allow).to_json();
            assert_eq!(got, case["expect"], "focus {name}");
        }
    }

    #[test]
    fn calls_vectors_sfu_url() {
        let v = vectors();
        let mut n = 0;
        for case in v["sfuUrl"].as_array().unwrap() {
            for pair in case["cases"].as_array().unwrap() {
                let (url, ok) = (pair[0].as_str().unwrap(), pair[1].as_bool().unwrap());
                assert_eq!(sfu_url_ok(url), ok, "sfuUrl {url:?}");
                n += 1;
            }
        }
        assert_eq!(n, 12);
    }

    #[test]
    fn calls_vectors_recipients() {
        let v = vectors();
        let defs = &v["defaults"];
        let cases = v["recipients"].as_array().unwrap();
        assert_eq!(cases.len(), 8);
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let room = field(case, defs, "room").as_str().unwrap();
            let now = field(case, defs, "now").as_u64().unwrap();
            let (me_user, me_dev) = field(case, defs, "me").as_str().unwrap().split_once('|').unwrap();
            let joined = field(case, defs, "joined").clone();
            // absent = joined; null = not known yet (never defaulted to join)
            let ms = |u: &str| match joined.get(u) {
                None => Some("join".to_string()),
                Some(Value::Null) => None,
                Some(x) => x.as_str().map(str::to_string),
            };
            let devices: HashMap<String, Vec<String>> = field(case, defs, "devices")
                .as_object()
                .unwrap()
                .iter()
                .map(|(u, l)| (u.clone(), strs(l)))
                .collect();
            let gone: Vec<(String, u64)> = field(case, defs, "sfuGone")
                .as_array()
                .unwrap()
                .iter()
                .map(|g| (g["id"].as_str().unwrap().to_string(), g["created_ts"].as_u64().unwrap()))
                .collect();
            let events: Vec<Value> = case["members"].as_array().unwrap().iter().map(|m| expand_member(m, room)).collect();
            let sessions = keyrules::live_sessions_from(&events, now);
            let got = key_recipients_v2(&sessions, &ms, me_user, me_dev, &devices, &gone);
            assert_eq!(got, strs(&case["expect"]), "recipients {name}");
        }
    }

    #[test]
    fn calls_vectors_rotation() {
        let v = vectors();
        let cases = v["rotation"].as_array().unwrap();
        assert_eq!(cases.len(), 9);
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let st = &case["start"];
            let mut r = strs(&st["R"]);
            let (mut p, out) = Planner::start(st["index"].as_u64().unwrap() as u8, st["t"].as_u64().unwrap(), &r);
            let js = |o: Vec<Action>| Value::Array(o.iter().map(Action::to_json).collect());
            assert_eq!(js(out), st["out"], "rotation {name} start");
            for step in case["steps"].as_array().unwrap() {
                let t = step["t"].as_u64().unwrap();
                if let Some(nr) = step.get("R") {
                    r = strs(nr);
                }
                if let Some(n) = step.get("note") {
                    let keys: Vec<u8> = n["keys"].as_array().unwrap().iter().map(|k| k.as_u64().unwrap() as u8).collect();
                    p.note_holders(&strs(&n["to"]), &keys);
                }
                let out = p.step(t, &r);
                assert_eq!(js(out), step.get("out").cloned().unwrap_or(json!([])), "rotation {name} t={t}");
            }
            let end = &case["end"];
            assert_eq!(json!(p.in_use()), end["inUse"], "rotation {name} inUse");
            assert_eq!(p.pending().map(|i| json!(i)).unwrap_or(Value::Null), end["pending"], "rotation {name} pending");
            assert_eq!(json!(p.announce_keys()), end["announceKeys"], "rotation {name} announceKeys");
        }
    }

    #[test]
    fn calls_vectors_intake() {
        let v = vectors();
        let cases = v["intake"].as_array().unwrap();
        assert_eq!(cases.len(), 3);
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let as_u8 = |x: &Value| x.as_array().unwrap().iter().map(|i| i.as_u64().unwrap() as u8).collect::<Vec<u8>>();
            let mut held = as_u8(&case["held"]);
            let mut evicted = Vec::new();
            for i in as_u8(&case["add"]) {
                evicted.extend(keyrules::note_index(&mut held, i));
            }
            assert_eq!(held, as_u8(&case["expectHeld"]), "intake {name}");
            assert_eq!(evicted, as_u8(&case["evicted"]), "intake {name} evicted");
        }
    }

    /// The byte copy equals the web's vectors (`TINHAT_WEB_DIR` = a tinhat-web tree, else `$HOME/tinhat-web`;
    /// skipped when neither has the file).
    #[test]
    fn calls_vectors_are_the_webs() {
        let dir = std::env::var_os("TINHAT_WEB_DIR").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("tinhat-web")));
        let Some(dir) = dir else { return };
        let path = dir.join("src/features/calls/testvectors/calls.json");
        match std::fs::read_to_string(&path) {
            Ok(theirs) => {
                assert!(VECTORS == theirs, "testvectors/calls.json differs from {}", path.display());
                eprintln!("calls vectors: byte-identical to {}", path.display());
            }
            Err(_) => eprintln!("SKIPPED: no web vectors at {}", path.display()),
        }
    }

    // ---- beyond the vectors ----

    #[test]
    fn addresses() {
        assert_eq!(normalize_focus_url("https://A.Test:443/livekit/jwt//"), Ok("https://a.test/livekit/jwt".into()));
        assert_eq!(normalize_focus_url("https://a.test"), Ok("https://a.test".into()));
        assert_eq!(normalize_focus_url("https://b.test:8448/x/"), Ok("https://b.test:8448/x".into()));
        assert_eq!(normalize_focus_url("https://[::1]:8443/jwt"), Ok("https://[::1]:8443/jwt".into()));
        assert_eq!(normalize_focus_url("http://a.test/jwt"), Err("insecure-scheme"));
        for bad in ["", "wss://a.test/jwt", "https://a.test/jwt#x", "https://a.test/ jwt", "https://u@a.test/", "https://:p@a.test/", "/livekit/jwt", "https://a.test/\u{7f}"] {
            assert_eq!(normalize_focus_url(bad), Err("bad-url"), "{bad:?}");
        }
        assert_eq!(normalize_focus_url(&format!("https://a.test/{}", "x".repeat(2040))), Err("bad-url"));
        assert_eq!(host_of("https://b.test:8448/livekit/jwt"), "b.test:8448");
        assert_eq!(host_of("https://a.test"), "a.test");
    }

    #[test]
    fn sheet_and_refusal_words() {
        let w = host_sheet_words("b.test", "@me:a.test", "Bob", "@bob:b.test");
        assert_eq!(w["title"], "Join a call hosted elsewhere?");
        assert_eq!(w["body"], "This call runs on b.test, not on your server. Joining gives b.test your IP address and a sign-in proof that says you are @me:a.test (valid for up to an hour). What you say and show stays end-to-end encrypted. Started by Bob (@bob:b.test).");
        assert_eq!((w["join"].as_str(), w["always"].as_str(), w["cancel"].as_str()), (Some("Join"), Some("Always trust b.test"), Some("Cancel")));
        assert!(host_sheet_words("b.test", "@me:a.test", "", "@bob:b.test")["body"].as_str().unwrap().ends_with("Started by @bob:b.test (@bob:b.test)."));
        assert_eq!(refusal_text("insecure-scheme"), "This call runs on an unencrypted address, so Tin Hat won't join it.");
        assert_eq!(refusal_text("bad-url"), "This call's address isn't valid, so Tin Hat won't join it.");
        assert_eq!(refusal_text("alias-mismatch"), "This call points at a different room, so Tin Hat won't join it.");
        // the Q1 switch as shipped: ask and trusted pass through
        let ask = FocusDecision { decision: "ask", call: true, service_url: Some("https://b.test".into()), alias: Some("!r".into()), host: Some("b.test".into()), advertiser: Some("@b:b".into()), reason: None };
        assert_eq!(apply_foreign_policy(ask.clone()), ask);
        assert_eq!(refusal_text("foreign-host"), "This call runs on another server, so Tin Hat won't join it.");
    }

    #[test]
    fn allow_list_round_trip() {
        let c = json!({"v": 1, "allow": ["b.test"], "x": true});
        let added = allow_list_with(&c, "c.test:8448", true);
        assert_eq!(allow_list_from(&added), vec!["b.test", "c.test:8448"]);
        assert_eq!(added["x"], true);
        assert_eq!(allow_list_from(&allow_list_with(&added, "b.test", false)), vec!["c.test:8448"]);
        assert_eq!(allow_list_from(&allow_list_with(&added, "b.test", true)), vec!["c.test:8448", "b.test"]);
        assert!(allow_list_from(&json!(null)).is_empty() && allow_list_from(&json!({"allow": "b.test"})).is_empty());
    }

    #[test]
    fn identities_and_the_sfu_gone_book() {
        assert_eq!(split_identity("@a:b.test:DEV"), Some(("@a:b.test".into(), "DEV".into())));
        assert_eq!(split_identity("@a:b.test:8448:DEV"), Some(("@a:b.test:8448".into(), "DEV".into())));
        for bad in ["abcdef0123", "@a:DEV", "@a:b.test:", "x:y:z"] {
            assert_eq!(split_identity(bad), None, "{bad}");
        }
        let s = |c: u64| LiveSession { sender: "@a:b.test".into(), device_id: "DEV".into(), state_key: String::new(), membership_id: String::new(), well_formed: true, created_ts: c, focus: None };
        let mut g = SfuGone::default();
        g.left("@a:b.test:DEV", &[s(5)]);
        g.left("@x:b.test:NOSESSION", &[s(5)]);
        assert_eq!(g.entries(), &[("@a:b.test:DEV".to_string(), 5)]);
        g.left("@a:b.test:DEV", &[s(6)]); // one entry per device, the latest session
        assert_eq!(g.entries(), &[("@a:b.test:DEV".to_string(), 6)]);
        g.joined("@a:b.test:DEV");
        assert!(g.entries().is_empty());
        g.left("@a:b.test:DEV", &[s(6)]);
        g.clear();
        assert!(g.entries().is_empty());
    }

    #[test]
    fn the_resend_ladder_after_a_switch() {
        let f = vec!["@c:a|C".to_string(), "@b:a|B".to_string(), "@b:a|B".to_string()];
        let to = vec!["@b:a|B".to_string(), "@c:a|C".to_string()];
        assert_eq!(resend_plan(&f, 7000), vec![(9000, to.clone()), (12000, to.clone()), (17000, to)]);
        assert!(resend_plan(&[], 7000).is_empty());
    }

    #[test]
    fn tick_switches_on_time_without_a_recipient_read() {
        let r = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let (mut p, _) = Planner::start(7, 0, &r(&["@b:a|B", "@c:a|C"]));
        p.step(1000, &r(&["@b:a|B"]));
        assert_eq!(p.step(4000, &r(&["@b:a|B"])), vec![Action::Mint { index: 8, to: r(&["@b:a|B"]) }]);
        assert_eq!(p.use_at(), Some(7000));
        assert!(p.tick(6999).is_empty());
        assert_eq!(p.tick(7000), vec![Action::Use(8)]);
        assert!(p.tick(7001).is_empty());
        assert_eq!((p.in_use(), p.pending(), p.use_at()), (8, None, None));
        assert!(p.step(7500, &r(&["@b:a|B"])).is_empty(), "the step after a tick doesn't switch again");
    }

    #[test]
    fn resend_obligations_are_per_index() {
        assert!(newer_index(8, 7) && !newer_index(7, 8) && newer_index(0, 254) && !newer_index(254, 0) && !newer_index(5, 5));
        // distance 127 (the odd ring's middle) is newer one way and older the other, never neither
        assert!(newer_index(127, 0) && !newer_index(0, 127) && newer_index(0, 128) && !newer_index(128, 0));
        let mut b = ResendBook::default();
        b.failed("X", 8);
        b.failed("X", 7); // an older failure never lowers what is owed
        b.delivered("X", 7); // a late success of the older key doesn't satisfy the newer one
        assert_eq!(b.to(), vec!["X"]);
        b.delivered("X", 8);
        assert!(b.is_empty());
        b.failed("Y", 254);
        b.failed("Y", 0); // wraps: 0 is newer
        b.delivered("Y", 254);
        assert_eq!(b.to(), vec!["Y"]);
        b.keep_only(&[1]); // key 0 superseded: its obligation ends
        assert!(b.is_empty());
    }

    #[test]
    fn planner_kill_switch_tracks_but_never_mints() {
        let r = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let (mut p, _) = Planner::start(7, 0, &r(&["@b:a|B", "@c:a|C"]));
        p.set_rotation(false);
        assert!(p.step(1000, &r(&["@b:a|B"])).is_empty());
        assert!(p.step(9000, &r(&["@b:a|B"])).is_empty());
        assert_eq!((p.in_use(), p.pending()), (7, None));
        assert_eq!(p.step(9500, &r(&["@b:a|B", "@d:a|D"])), vec![Action::Announce { keys: vec![7], to: r(&["@d:a|D"]) }]);
    }

    #[test]
    fn planner_note_holders_makes_a_ladder_recipient_a_holder() {
        let r = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let (mut p, _) = Planner::start(7, 0, &r(&["@b:a|B"]));
        // the steady loop keyed X between two planner steps, and X is already gone at the next one
        p.note_holders(&r(&["@x:a|X"]), &[7]);
        assert!(p.step(1000, &r(&["@b:a|B"])).is_empty());
        assert_eq!(p.step(4000, &r(&["@b:a|B"])), vec![Action::Mint { index: 8, to: r(&["@b:a|B"]) }]);
        assert_eq!(p.next_tick_ms(), 1000);
        assert_eq!(p.step(7000, &r(&["@b:a|B"])), vec![Action::Use(8)]);
        assert_eq!(p.next_tick_ms(), 5000);
    }

    /// 12.0.5 review item 1: eligibility is re-checked at each dispatch, so a leave that lands while an earlier batch is
    /// being sent drops the later one; a failed batch doesn't stop the rest.
    #[test]
    fn dispatch_rechecks_each_batch_at_its_own_send() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let eligible = std::rc::Rc::new(std::cell::RefCell::new(vec!["a".to_string(), "b".to_string(), "c".to_string(), "d".to_string()]));
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        let (e1, e2, l2) = (eligible.clone(), eligible.clone(), log.clone());
        let rep = rt.block_on(dispatch_checked(
            vec!["a", "b", "c", "d"],
            |t| t.to_string(),
            move |id| {
                let ok = e1.borrow().contains(&id);
                async move { ok }
            },
            move |t: &str| {
                l2.borrow_mut().push(t.to_string());
                let e = e2.clone();
                let t = t.to_string();
                async move {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await; // the batch is in flight…
                    if t == "a" {
                        e.borrow_mut().retain(|x| x != "b"); // …and b's kick syncs meanwhile
                    }
                    t != "c" // c's batch fails (retries exhausted)
                }
            },
        ));
        assert_eq!(rep.sent, vec!["a", "d"]);
        assert_eq!(rep.dropped, vec!["b"]);
        assert_eq!(rep.failed, vec!["c"]);
        assert_eq!(*log.borrow(), vec!["a", "c", "d"]);
    }

    /// 12.0.5 review item 3: an unknown member never holds up keys to the known ones; it is keyed once the member
    /// list says "join" (the planner's newcomer announce).
    #[test]
    fn an_unknown_member_is_keyed_after_the_member_sync_without_delaying_the_rest() {
        let ev = |s: &str, d: &str| expand_member(&json!({"s": s, "d": d, "c": 1_761_399_000_000u64}), "!call:a.test");
        let sessions = keyrules::live_sessions_from(&[ev("@alice:a.test", "A"), ev("@carol:a.test", "C")], 1_761_400_000_000);
        let none = HashMap::new();
        let before = |u: &str| (u == "@alice:a.test").then(|| "join".to_string());
        let r0 = key_recipients_v2(&sessions, &before, "@me:a.test", "M", &none, &[]);
        assert_eq!(r0, vec!["@alice:a.test|A"]);
        let (mut p, out) = Planner::start(3, 0, &r0);
        assert_eq!(out, vec![Action::Announce { keys: vec![3], to: r0.clone() }]);
        let after = |_: &str| Some("join".to_string());
        let r1 = key_recipients_v2(&sessions, &after, "@me:a.test", "M", &none, &[]);
        assert_eq!(p.step(500, &r1), vec![Action::Announce { keys: vec![3], to: vec!["@carol:a.test|C".into()] }]);
    }

    #[test]
    fn running_session_is_a_total_order() {
        let mk = |s: &str, d: &str, c: u64| LiveSession { sender: s.into(), device_id: d.into(), state_key: String::new(), membership_id: String::new(), well_formed: true, created_ts: c, focus: Some((format!("https://{d}.test"), String::new())) };
        let a = [mk("@b:x", "B", 1), mk("@a:x", "Z", 1), mk("@a:x", "Y", 1), mk("@c:x", "C", 0)];
        assert_eq!(running_session(&a).unwrap().device_id, "C");
        let mut b = a[..3].to_vec();
        for _ in 0..3 {
            b.rotate_left(1);
            assert_eq!(running_session(&b).unwrap().device_id, "Y", "order-independent");
        }
    }
}
