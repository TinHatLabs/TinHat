//! Pad on the phone, Step A (share claude/tinhat-pad/PAD-PHONE/DESIGN.md): ONE call tile per PERSON, and the phone/Pad
//! chip. The web's src/features/calls/persontiles.ts, same rules, same vectors (testvectors/persontiles.json =
//! web persontiles.vectors.json, byte-identical).
//!
//! The trust rule: a participant belongs to person P ONLY when the SERVER says so — the attribute `tinhat.person` = P,
//! which a 0.3.13+ Tin Hat Server mints into every call token (canUpdateOwnMetadata false: no participant can set or
//! change its own; a client data packet, its metadata or its name never count) — AND its identity is exactly
//! `P:<device>` (device `[A-Za-z0-9_+/-]{1,64}`: the server's identity.mjs DEVICE_RE, 0.3.12) with an optional exact `~watch-<16 lowercase hex>`. No attribute (an
//! older server, a session from before the server's 0.3.13 key rotation) or anything else = nobody's: its own tile.
//!
//! Step B (12.1.10 / server 0.3.14): the phone's Mic and Camera modes (`tinhat.mode=mic` + `~pad-mic-<16 hex>`,
//! `tinhat.mode=camera` + `~pad-cam-<16 hex>`); a person's chips aggregate ("mic+camera"); a Camera-mode phone has no
//! mic; a Pad camera is the shown camera unless the viewer picked another; my own other devices' mics are never played
//! ([`play_mic_of`]); the call chimes follow PEOPLE ([`PersonChimes`]).
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};

/// JavaScript's `\s` (the server's MXID_RE and the web's persontiles.ts use it): Rust's `char::is_whitespace` differs
/// (it has U+0085, lacks U+FEFF), so the set is spelled out — the shared vectors pin both edges.
fn js_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}')
}

fn is_mxid(s: &str) -> bool {
    // @local:server[:port] | @local:[v6][:port] — the server's identity.mjs MXID_RE, lengths in UTF-16 code units as JS
    let Some(rest) = s.strip_prefix('@') else { return false };
    let Some(colon) = rest.find(':') else { return false };
    let (local, server) = (&rest[..colon], &rest[colon + 1..]);
    let units = local.encode_utf16().count();
    if units == 0 || units > 255 || local.chars().any(|c| c == ':' || js_space(c)) {
        return false;
    }
    let port_ok = |p: &str| !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit());
    if let Some(v6) = server.strip_prefix('[') {
        let Some(end) = v6.find(']') else { return false };
        let (addr, after) = (&v6[..end], &v6[end + 1..]);
        let addr_ok = (2..=45).contains(&addr.len()) && addr.bytes().all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.');
        return addr_ok && (after.is_empty() || after.strip_prefix(':').is_some_and(port_ok));
    }
    let (host, port) = match server.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (server, None),
    };
    let host_ok = (1..=255).contains(&host.len()) && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    host_ok && port.is_none_or(port_ok)
}

fn hex16(h: &str) -> bool {
    h.len() == 16 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The exact Pad suffix `~watch-<16hex>` / `~pad-mic-<16hex>` / `~pad-cam-<16hex>` -> its mode tag ("watch", "mic",
/// "cam"), or None.
fn pad_suffix(w: &str) -> Option<&'static str> {
    if w.strip_prefix("~watch-").is_some_and(hex16) {
        Some("watch")
    } else if w.strip_prefix("~pad-mic-").is_some_and(hex16) {
        Some("mic")
    } else if w.strip_prefix("~pad-cam-").is_some_and(hex16) {
        Some("cam")
    } else {
        None
    }
}

fn device_part_ok(s: &str) -> bool {
    let (dev, suffix) = match s.find('~') {
        Some(i) => (&s[..i], Some(&s[i..])),
        None => (s, None),
    };
    let dev_ok = (1..=64).contains(&dev.len()) && dev.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'/' | b'-'));
    dev_ok && suffix.is_none_or(|w| pad_suffix(w).is_some())
}

/// The Pad suffix of an identity by the identity alone (the last '~' on): its tag, or None.
fn suffix_tag(identity: &str) -> Option<&'static str> {
    identity.rfind('~').and_then(|i| pad_suffix(&identity[i..]))
}

/// The person the server vouches for (its mxid), or "" (nobody's).
pub fn person_of(identity: &str, attrs: &HashMap<String, String>) -> String {
    let Some(p) = attrs.get("tinhat.person") else { return String::new() };
    if !is_mxid(p) {
        return String::new();
    }
    match identity.strip_prefix(p.as_str()).and_then(|r| r.strip_prefix(':')) {
        Some(rest) if device_part_ok(rest) => p.clone(),
        _ => String::new(),
    }
}

/// The Pad chip of one participant ("" = none, "watch", "mic", "camera"): `tinhat.pad=phone` + a `tinhat.mode` whose
/// identity suffix matches (watch ↔ `~watch-`, mic ↔ `~pad-mic-`, camera ↔ `~pad-cam-`).
pub fn chip_of(identity: &str, attrs: &HashMap<String, String>) -> &'static str {
    if person_of(identity, attrs).is_empty() {
        return "";
    }
    if attrs.get("tinhat.pad").map(String::as_str) != Some("phone") {
        return "";
    }
    match (attrs.get("tinhat.mode").map(String::as_str), suffix_tag(identity)) {
        (Some("watch"), Some("watch")) => "watch",
        (Some("mic"), Some("mic")) => "mic",
        (Some("camera"), Some("cam")) => "camera",
        _ => "",
    }
}

/// A person's chips, each once, in the order watch, mic, camera, joined by "+".
pub fn pad_modes_of<'a>(chips: impl IntoIterator<Item = &'a str>) -> String {
    let set: HashSet<&str> = chips.into_iter().collect();
    ["watch", "mic", "camera"].iter().filter(|c| set.contains(**c)).copied().collect::<Vec<_>>().join("+")
}

/// A device publishing no microphone by its mode (a watcher, a Camera-mode phone).
fn no_mic(c: &str) -> bool {
    c == "watch" || c == "camera"
}

fn s<'a>(t: &'a Value, k: &str) -> &'a str {
    t.get(k).and_then(Value::as_str).unwrap_or("")
}
fn b(t: &Value, k: &str) -> bool {
    t.get(k).and_then(Value::as_bool).unwrap_or(false)
}
fn is_share(t: &Value) -> bool {
    s(t, "kind") == "screenshare" || b(t, "screenshare")
}
/// The tile with the fold's fields at their defaults (members [], pad "", othersLive false, alts [], altOf "",
/// partlyMuted false).
fn base(mut t: Value) -> Value {
    if let Some(o) = t.as_object_mut() {
        o.insert("members".into(), Value::Array(vec![]));
        o.insert("pad".into(), Value::from(""));
        o.insert("othersLive".into(), Value::from(false));
        o.insert("alts".into(), Value::Array(vec![]));
        o.insert("altOf".into(), Value::from(""));
        o.insert("partlyMuted".into(), Value::from(false));
    }
    t
}
fn set(t: &mut Value, k: &str, v: Value) {
    if let Some(o) = t.as_object_mut() {
        o.insert(k.into(), v);
    }
}

/// Fold the call's tiles into one per person (after the companions fold) — persontiles.ts foldPersons, rule for rule.
/// `pick` = the viewer's camera choice per person (tile id).
pub fn fold_persons(
    tiles: Vec<Value>,
    own_identity: &str,
    attrs_of: &dyn Fn(&str) -> HashMap<String, String>,
    pick: &HashMap<String, String>,
) -> Vec<Value> {
    let own = own_identity.rfind(':').filter(|&i| i > 0).map(|i| own_identity[..i].to_string()).unwrap_or_default();
    let key_of = |t: &Value| -> String {
        let p = s(t, "participant");
        if p == "You" { String::new() } else { person_of(p, &attrs_of(p)) }
    };
    let has_self = tiles.iter().any(|t| s(t, "participant") == "You");
    let mut members: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut chips: HashMap<String, Vec<&'static str>> = HashMap::new();
    let mut cam_chip: HashMap<String, &'static str> = HashMap::new();
    let mut all_muted: HashMap<String, bool> = HashMap::new();
    let mut any_live: HashMap<String, bool> = HashMap::new();
    let mut sm_all: HashMap<String, bool> = HashMap::new();
    let mut sm_any: HashMap<String, String> = HashMap::new();
    let mut cams: HashMap<String, Vec<String>> = HashMap::new();
    let mut anchor_av: HashMap<String, (String, bool)> = HashMap::new();
    for t in &tiles {
        let k = key_of(t);
        if k.is_empty() || is_share(t) {
            continue;
        }
        let p = s(t, "participant").to_string();
        let c = chip_of(&p, &attrs_of(&p));
        if !c.is_empty() {
            chips.entry(k.clone()).or_default().push(c);
        }
        let watcher = no_mic(c); // a watcher / a Camera-mode phone: no microphone
        // a watcher publishes nothing (the server's token): never a live mic (review r2)
        let mic = !b(t, "muted") && !watcher;
        let e = all_muted.entry(k.clone()).or_insert(true);
        *e = *e && !mic;
        let l = any_live.entry(k.clone()).or_insert(false);
        *l = *l || mic;
        // a watcher can't be muted by a moderator (the server answers 409): it never decides "moderated" (a045 review)
        if !watcher {
            let sm = b(t, "serverMuted");
            let e = sm_all.entry(k.clone()).or_insert(true);
            *e = *e && sm;
            if sm && !sm_any.contains_key(&k) {
                sm_any.insert(k.clone(), s(t, "serverMutedBy").to_string());
            }
        }
        // the tile a person without a camera is shown by: their first NON-watcher avatar, else their first avatar
        if s(t, "kind") == "avatar" && (!anchor_av.contains_key(&k) || (anchor_av.get(&k).is_some_and(|a| a.1) && !watcher)) {
            anchor_av.insert(k.clone(), (s(t, "id").to_string(), watcher));
        }
        if s(t, "kind") == "video" {
            cams.entry(k.clone()).or_default().push(s(t, "id").to_string());
            cam_chip.insert(s(t, "id").to_string(), c);
        }
        members.entry(k).or_default().insert(p);
    }
    let chip: HashMap<String, String> = chips.iter().map(|(k, v)| (k.clone(), pad_modes_of(v.iter().copied()))).collect();
    // Step B: a Pad Camera-mode camera first (the lowest such tile id)
    let pad_cam = |sorted: &[String]| -> Option<String> { sorted.iter().find(|x| cam_chip.get(x.as_str()).copied() == Some("camera")).cloned() };
    // MY cameras (review r2): my self-view ("You") is one of them, shown unless I picked another
    let self_cam: String = if !own.is_empty() && has_self {
        tiles.iter().find(|t| s(t, "participant") == "You" && s(t, "kind") == "video").map(|t| s(t, "id").to_string()).unwrap_or_default()
    } else {
        String::new()
    };
    let mut shown_cam: HashMap<String, String> = HashMap::new();
    for (k, ids) in &cams {
        let mut sorted = ids.clone();
        sorted.sort();
        let chosen = if *k == own && !self_cam.is_empty() {
            pick.get(k).filter(|p| **p == self_cam || sorted.contains(p)).cloned().or_else(|| pad_cam(&sorted)).unwrap_or_else(|| self_cam.clone())
        } else {
            pick.get(k).filter(|p| sorted.contains(p)).cloned().or_else(|| pad_cam(&sorted)).unwrap_or_else(|| sorted[0].clone())
        };
        shown_cam.insert(k.clone(), chosen);
    }
    let my_cams: Vec<String> = if !own.is_empty() && !self_cam.is_empty() {
        std::iter::once(self_cam.clone()).chain(cams.get(&own).cloned().unwrap_or_default()).collect()
    } else {
        vec![]
    };
    let my_alts = |shown: &str| -> Vec<String> {
        let mut v: Vec<String> = my_cams.iter().filter(|x| x.as_str() != shown).cloned().collect();
        v.sort();
        v
    };
    let my_members: Vec<String> = if !own.is_empty() && has_self { members.get(&own).map(|b| b.iter().cloned().collect()).unwrap_or_default() } else { vec![] };
    let my_pad: String = if own.is_empty() { String::new() } else { chip.get(&own).cloned().unwrap_or_default() };
    let my_live = !own.is_empty() && any_live.get(&own).copied().unwrap_or(false);
    let mine = |x: &mut Value| {
        set(x, "members", Value::from(my_members.clone()));
        set(x, "pad", Value::from(my_pad.as_str()));
        set(x, "othersLive", Value::from(my_live));
    };
    let mut anchored: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(tiles.len());
    for t in tiles {
        if s(&t, "participant") == "You" {
            let shown = if own.is_empty() { None } else { shown_cam.get(&own).cloned() };
            let is_self_cam = !self_cam.is_empty() && s(&t, "id") == self_cam;
            let mut x = base(t);
            mine(&mut x);
            if let (true, Some(sh)) = (is_self_cam, &shown) {
                if *sh != self_cam {
                    set(&mut x, "altOf", Value::from(sh.as_str()));
                } else {
                    set(&mut x, "alts", Value::from(my_alts(&self_cam)));
                }
            }
            out.push(x);
            continue;
        }
        let k = key_of(&t);
        if k.is_empty() || is_share(&t) {
            out.push(base(t));
            continue;
        }
        if k == own && has_self {
            if s(&t, "kind") == "avatar" {
                continue; // my other device: on my self tile
            }
            if let (false, Some(sh), "video") = (self_cam.is_empty(), shown_cam.get(&own).cloned(), s(&t, "kind")) {
                // one of MY cameras: shown in place of my self-view only when I picked it
                let mut x = base(t);
                if s(&x, "id") == sh {
                    mine(&mut x);
                    set(&mut x, "alts", Value::from(my_alts(&sh)));
                } else {
                    set(&mut x, "altOf", Value::from(sh.as_str()));
                }
                out.push(x);
                continue;
            }
            out.push(base(t));
            continue;
        }
        let kind = s(&t, "kind").to_string();
        let cam = shown_cam.get(&k).cloned();
        if let Some(c) = &cam {
            if kind == "avatar" {
                continue;
            }
            if s(&t, "id") != c {
                let mut x = base(t);
                set(&mut x, "altOf", Value::from(c.as_str()));
                out.push(x);
                continue;
            }
        } else if kind != "avatar" || anchored.contains(&k) || anchor_av.get(&k).map(|a| a.0.as_str()) != Some(s(&t, "id")) {
            if kind == "avatar" {
                continue; // another avatar of the same person (a watcher's when they have a computer)
            }
            out.push(base(t));
            continue;
        }
        anchored.insert(k.clone());
        let m: Vec<String> = members.get(&k).map(|b| b.iter().cloned().collect()).unwrap_or_default();
        let m = if m.len() > 1 { m } else { vec![] };
        let all = sm_all.get(&k).copied().unwrap_or(false);
        let some = sm_any.contains_key(&k);
        let own_by = s(&t, "serverMutedBy").to_string();
        let own_sm = b(&t, "serverMuted");
        let by = if all || !own_sm { if own_by.is_empty() { sm_any.get(&k).cloned().unwrap_or_default() } else { own_by } } else { own_by };
        let alts: Vec<String> = match &cam {
            Some(c) => {
                let mut v: Vec<String> = cams.get(&k).cloned().unwrap_or_default().into_iter().filter(|x| x != c).collect();
                v.sort();
                v
            }
            None => vec![],
        };
        let muted = all_muted.get(&k).copied();
        let mut x = base(t);
        set(&mut x, "members", Value::from(m));
        set(&mut x, "pad", Value::from(chip.get(&k).cloned().unwrap_or_default()));
        if let Some(mu) = muted {
            set(&mut x, "muted", Value::from(mu));
        }
        set(&mut x, "serverMuted", Value::from(all));
        set(&mut x, "serverMutedBy", Value::from(by));
        set(&mut x, "partlyMuted", Value::from(some && !all));
        set(&mut x, "alts", Value::from(alts));
        out.push(x);
    }
    out
}

// ---- Step B: what is played, and the call chimes ---------------------------------------------------------------------

/// Is a remote MICROPHONE track of this participant played here? Not when it is one of MY other devices (server-vouched:
/// tinhat.person = my mxid and the exact identity form). Everyone else's: yes.
pub fn play_mic_of(identity: &str, attrs: &HashMap<String, String>, own_identity: &str) -> bool {
    let own = own_identity.rfind(':').filter(|&i| i > 0).map(|i| &own_identity[..i]).unwrap_or("");
    !(!own.is_empty() && identity != own_identity && person_of(identity, attrs) == own)
}

/// Whether this identity is a Pad connection (Watch, Mic, Camera), by the identity alone.
pub fn is_pad_identity(identity: &str) -> bool {
    suffix_tag(identity).is_some()
}

/// The person a call chime is about: the server-vouched person; else the identity's own mxid (a Pad suffix dropped,
/// everything before the LAST ':' while that still holds a ':'); else the identity.
pub fn chime_person(identity: &str, attrs: &HashMap<String, String>) -> String {
    let p = person_of(identity, attrs);
    if !p.is_empty() {
        return p;
    }
    let base = match identity.rfind('~') {
        Some(i) if pad_suffix(&identity[i..]).is_some() => &identity[..i],
        _ => identity,
    };
    match base.rfind(':') {
        Some(i) if i > 0 && base[..i].contains(':') => base[..i].to_string(),
        _ => identity.to_string(),
    }
}

/// Step B: a server-vouched Pad Mic / Camera connection (`chip_of` mic | camera) sends its media keys to-device under its
/// device's own identity `<mxid>:<device>`, so the receiver installs that base identity's keys (every slot, now and
/// later) under the Pad identity too. -> the base identity, or "" (not an alias: a watcher publishes nothing).
pub fn key_alias_base(identity: &str, attrs: &HashMap<String, String>) -> String {
    match chip_of(identity, attrs) {
        "mic" | "camera" => identity.rfind('~').map(|i| identity[..i].to_string()).unwrap_or_default(),
        _ => String::new(),
    }
}

/// How long a Pad connection's leave waits for the same person's next connection (ms).
pub const PAD_LEAVE_GRACE_MS: u64 = 5_000;

/// The call chimes follow PEOPLE (persontiles.ts PersonChimes, 2026-11-06): a join chimes only for a person with
/// no other connection in the call (me included), a leave only for their last one; a Pad connection that was their last
/// waits [`PAD_LEAVE_GRACE_MS`] and their return in that time cancels both chimes.
#[derive(Default, Debug)]
pub struct PersonChimes {
    /// person -> when their held-back leave chime is due
    pub pending: HashMap<String, u64>,
    /// leave chimes owed by people who came back only AFTER their grace (a late timer): tick plays them
    pub owed: u32,
}

impl PersonChimes {
    pub fn join(&mut self, identity: &str, attrs_of: &dyn Fn(&str) -> HashMap<String, String>, present: &[String], now: u64) -> &'static str {
        let p = chime_person(identity, &attrs_of(identity));
        if let Some(due) = self.pending.remove(&p) {
            if now < due {
                return ""; // back within the grace: a switch, not a new arrival
            }
            self.owed += 1; // back after it: their leave was real (owed to tick) and so is this arrival
            return "join";
        }
        if present.iter().any(|o| o != identity && chime_person(o, &attrs_of(o)) == p) { "" } else { "join" }
    }

    pub fn leave(&mut self, identity: &str, attrs_of: &dyn Fn(&str) -> HashMap<String, String>, present: &[String], now: u64) -> &'static str {
        let p = chime_person(identity, &attrs_of(identity));
        if present.iter().any(|o| o != identity && chime_person(o, &attrs_of(o)) == p) {
            return "";
        }
        if is_pad_identity(identity) {
            self.pending.insert(p, now + PAD_LEAVE_GRACE_MS);
            return "";
        }
        "leave"
    }

    /// The held-back leaves that are due: each person still away = one leave chime.
    pub fn tick(&mut self, attrs_of: &dyn Fn(&str) -> HashMap<String, String>, present: &[String], now: u64) -> u32 {
        let due: Vec<String> = self.pending.iter().filter(|(_, d)| **d <= now).map(|(p, _)| p.clone()).collect();
        let mut n = std::mem::take(&mut self.owed);
        for p in due {
            self.pending.remove(&p);
            if !present.iter().any(|o| chime_person(o, &attrs_of(o)) == p) {
                n += 1;
            }
        }
        n
    }

    pub fn clear(&mut self) {
        self.pending.clear();
        self.owed = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vectors() -> Value {
        serde_json::from_str(include_str!("testvectors/persontiles.json")).expect("vectors")
    }
    fn attrs(v: &Value) -> HashMap<String, String> {
        v.as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect()).unwrap_or_default()
    }

    #[test]
    fn person_and_chip_every_vector() {
        for c in vectors()["person"].as_array().unwrap() {
            let id = c["identity"].as_str().unwrap();
            let a = attrs(&c["attrs"]);
            assert_eq!(person_of(id, &a), c["person"].as_str().unwrap(), "{id:?}");
            assert_eq!(chip_of(id, &a), c["chip"].as_str().unwrap(), "{id:?}");
        }
    }

    #[test]
    fn fold_every_vector() {
        for c in vectors()["fold"].as_array().unwrap() {
            let table: HashMap<String, HashMap<String, String>> =
                c["attrs"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), attrs(v))).collect();
            let tiles: Vec<Value> = c["tiles"].as_array().unwrap().iter().map(|t| {
                let mut t = t.clone();
                let ss = t["kind"] == "screenshare";
                t["screenshare"] = Value::from(ss);
                t
            }).collect();
            let pick: HashMap<String, String> = c.get("pick").and_then(Value::as_object).map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect()).unwrap_or_default();
            let out = fold_persons(tiles, c["own"].as_str().unwrap(), &|id| table.get(id).cloned().unwrap_or_default(), &pick);
            let shape = |t: &Value| serde_json::json!({"id": t["id"], "members": t["members"], "pad": t["pad"], "othersLive": t["othersLive"], "alts": t["alts"], "altOf": t["altOf"], "serverMuted": t.get("serverMuted").and_then(Value::as_bool).unwrap_or(false), "partlyMuted": t["partlyMuted"], "muted": t["muted"]});
            let got: Vec<Value> = out.iter().map(shape).collect();
            let want: Vec<Value> = c["out"].as_array().unwrap().iter().map(shape).collect();
            assert_eq!(Value::from(got), Value::from(want), "{}", c["name"]);
        }
    }

    #[test]
    fn the_webs_copy_of_the_vectors_is_byte_identical() {
        let mine = include_bytes!("testvectors/persontiles.json");
        let home = std::env::var("HOME").unwrap_or_default();
        for d in [std::env::var("TINHAT_WEB_DIR").unwrap_or_default(), format!("{home}/tinhat-web")] {
            let p = format!("{d}/src/features/calls/persontiles.vectors.json");
            if let Ok(b) = std::fs::read(&p) {
                assert_eq!(&b[..], &mine[..], "{p}");
                return;
            }
        }
    }

    #[test]
    fn play_mic_every_vector() {
        for c in vectors()["play"].as_array().unwrap() {
            let a = attrs(&c["attrs"]);
            assert_eq!(play_mic_of(c["identity"].as_str().unwrap(), &a, c["own"].as_str().unwrap()), c["expect"].as_bool().unwrap(), "{}", c["name"]);
        }
    }

    #[test]
    fn person_chimes_every_vector() {
        for c in vectors()["chimes"].as_array().unwrap() {
            let table: HashMap<String, HashMap<String, String>> =
                c["attrs"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), attrs(v))).collect();
            let attrs_of = |id: &str| table.get(id).cloned().unwrap_or_default();
            let mut ch = PersonChimes::default();
            for (i, st) in c["steps"].as_array().unwrap().iter().enumerate() {
                let present: Vec<String> = st["present"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
                let now = st["now"].as_u64().unwrap();
                let id = st["id"].as_str().unwrap();
                let got = match st["op"].as_str().unwrap() {
                    "join" => Value::from(ch.join(id, &attrs_of, &present, now)),
                    "leave" => Value::from(ch.leave(id, &attrs_of, &present, now)),
                    _ => Value::from(ch.tick(&attrs_of, &present, now)),
                };
                assert_eq!(got, st["expect"], "{} — step {i}", c["name"]);
            }
        }
    }

    #[test]
    fn key_alias_every_vector() {
        for c in vectors()["keyAlias"].as_array().unwrap() {
            let id = c["identity"].as_str().unwrap();
            assert_eq!(key_alias_base(id, &attrs(&c["attrs"])), c["expect"].as_str().unwrap(), "{id:?}");
        }
    }
}
