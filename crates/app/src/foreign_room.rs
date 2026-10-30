//! Rooms that live on another server (share claude/tinhat-server/MATRIX-FRONTEND-DESIGN.md B0/B1/B5, the must-fix
//! round). The web's `src/core/matrix/foreign.ts` (+ `foreignHere.ts` for the store reads) applies the same rules and
//! both replay `testvectors/foreign-room.json` (owned by the web, copied byte for byte).
//!
//! - [`server_part`]: `@a:example.org` / `!x:example.org` / `#r:example.org` → `example.org`; a v12 `!opaque` id → `""`.
//! - [`room_origin`]: the `m.room.create` sender's server (a label only: it carries no trust).
//! - [`is_managed`]: may Tin Hat change this room's power levels? Only when the create sender and every v12
//!   `additional_creators` entry are on my own server (B0 `tinhatManaged`).
//! - [`pick_via`]: the servers a join asks through: the link's `via`, the space child's `via`, the target's own server
//!   part, then hints (the inviter's / the child event sender's server). Never a server read off a v12 opaque id.
//! - [`pl_write_plan`]: the one power-level write `appstate::ensure_writable` may make (or why not).

use matrix_sdk::ruma::events::StateEventType;
use matrix_sdk::Room;
use serde_json::Value;

/// matrix-js-sdk sends only the first 3 `via`s; the desktop sends the same 3.
const MAX_VIA: usize = 3;

/// A valid server name: a hostname or a bracketed IPv6 literal (a real address, as `Ipv6Addr` reads it — the web's
/// validIpv6 mirrors it), an optional port 1–65535 (the web's validServer).
pub fn valid_server(s: &str) -> bool {
    let (host, port) = if let Some(rest) = s.strip_prefix('[') {
        let Some((inner, after)) = rest.split_once(']') else { return false };
        if inner.parse::<std::net::Ipv6Addr>().is_err() {
            return false;
        }
        match after {
            "" => ("x", None),
            a => match a.strip_prefix(':') {
                Some(p) => ("x", Some(p)),
                None => return false,
            },
        }
    } else {
        match s.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (s, None),
        }
    };
    if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return false;
    }
    match port {
        None => true,
        Some(p) => {
            (1..=5).contains(&p.len()) && p.chars().all(|c| c.is_ascii_digit()) && p.parse::<u32>().is_ok_and(|n| (1..=65535).contains(&n))
        }
    }
}

/// The server part of a user id, pre-v12 room id or alias (after the first `:`); `""` for none or an invalid name.
pub fn server_part(id: &str) -> String {
    match id.split_once(':') {
        Some((_, s)) if valid_server(s) => s.to_string(),
        _ => String::new(),
    }
}

/// Where a room is from: the `m.room.create` sender's server, from the create event as JSON (`{sender, content}`).
pub fn room_origin(create: &Value) -> Option<String> {
    let sender = create.get("sender")?.as_str()?;
    if !sender.starts_with('@') {
        return None;
    }
    Some(server_part(sender)).filter(|s| !s.is_empty())
}

/// B0 `tinhatManaged`: Tin Hat may change power levels here only when every creator is on my own server.
pub fn is_managed(create: &Value, own_server: &str) -> bool {
    if own_server.is_empty() || room_origin(create).as_deref() != Some(own_server) {
        return false;
    }
    match create.get("content").and_then(|c| c.get("additional_creators")) {
        None => true,
        Some(Value::Array(extra)) => extra.iter().all(|u| {
            u.as_str().is_some_and(|u| u.starts_with('@') && server_part(u) == own_server)
        }),
        Some(_) => false,
    }
}

/// B1 `pickVia` (joining): link via, space-child via, the target's own server part, hints. Deduped, valid, ≤ 3.
pub fn pick_via(target: &str, link_via: &[&str], child_via: &[&str], hints: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let own = server_part(target);
    let all = link_via.iter().chain(child_via).copied().chain(std::iter::once(own.as_str())).chain(hints.iter().copied());
    for v in all {
        if out.len() >= MAX_VIA {
            break;
        }
        if valid_server(v) && !out.iter().any(|o| o == v) {
            out.push(v.to_string());
        }
    }
    out
}

/// The types `ensure_writable` may ever lower: Tin Hat's own state and call membership — never a Matrix type.
pub fn lowerable(t: &str) -> bool {
    t.starts_with("dev.tinhat.") || t == "org.matrix.msc3401.call.member" || t == "m.call.member"
}

/// A power level as the room holds it: a number, or (older room versions) an integer string like "50" / "-10" (the
/// web's level).
pub fn level(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        // no whitespace: the web's exact rule
        Value::String(s) => {
            let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
            if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            s.parse::<f64>().ok()
        }
        _ => None,
    }
}

/// What [`pl_write_plan`] decided.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// "foreign" | "no-power-levels" | "nothing-to-do" | "not-allowed" | "write" (the web's PlanReason)
    pub reason: &'static str,
    /// the types this write lowers to 0 (empty unless reason = "write")
    pub types: Vec<String>,
    /// the whole new `m.room.power_levels` content (None unless reason = "write")
    pub content: Option<Value>,
}

/// B0/B5 rule 1 as one pure decision (the web's plWritePlan): a room on another server never gets a write; else lower
/// each allowed, not-yet-done type whose effective level (`events[t]`, else `state_default`, else 50) is above 0 — a
/// level at or below 0 is left as it is.
pub fn pl_write_plan(create: &Value, own: &str, pl: Option<&Value>, types: &[&str], can_edit: bool, done: &[String]) -> Plan {
    let none = |reason| Plan { reason, types: Vec::new(), content: None };
    if !is_managed(create, own) {
        return none("foreign");
    }
    let Some(pl) = pl.filter(|p| p.is_object()) else { return none("no-power-levels") };
    let empty = serde_json::Map::new();
    let events = pl.get("events").and_then(Value::as_object).unwrap_or(&empty);
    let sd = pl.get("state_default").and_then(level).unwrap_or(50.0);
    let mut out: Vec<String> = Vec::new();
    for t in types {
        if !lowerable(t) || done.iter().any(|d| d == t) || out.iter().any(|o| o == t) {
            continue;
        }
        if events.get(*t).and_then(level).unwrap_or(sd) > 0.0 {
            out.push((*t).to_string());
        }
    }
    if out.is_empty() {
        return none("nothing-to-do");
    }
    if !can_edit {
        return none("not-allowed");
    }
    let mut next = pl.clone();
    let mut ev = events.clone();
    for t in &out {
        ev.insert(t.clone(), Value::from(0));
    }
    next["events"] = Value::Object(ev);
    Plan { reason: "write", types: out, content: Some(next) }
}

// ---- store reads (the web's foreignHere.ts) ----------------------------------------------------

/// The room's `m.room.create` event as JSON (`{sender, content, …}`), from the local store.
pub async fn create_event(room: &Room) -> Option<Value> {
    let raw = room.get_state_event(StateEventType::RoomCreate, "").await.ok()??;
    crate::appstate::raw_to_value(&raw)
}

/// [`is_managed`] for a room in the local store (my server = my user id's). No create state → false.
pub async fn managed_here(room: &Room) -> bool {
    match create_event(room).await {
        Some(c) => is_managed(&c, room.own_user_id().server_name().as_str()),
        None => false,
    }
}

/// The origin of a room in the local store, for log lines ("another server" when unknown).
pub async fn origin_here(room: &Room) -> String {
    create_event(room).await.and_then(|c| room_origin(&c)).unwrap_or_else(|| "another server".to_string())
}

/// Every joined space's `m.space.child` `via` for this room, plus those events' senders' servers (as hints).
pub async fn child_via_for(client: &matrix_sdk::Client, room_id: &str) -> (Vec<String>, Vec<String>) {
    let (mut via, mut senders) = (Vec::new(), Vec::new());
    // a fixed order (by space id), so both apps ask the same servers first whatever order their stores keep
    let mut spaces = client.joined_rooms();
    spaces.sort_by(|a, b| a.room_id().as_str().cmp(b.room_id().as_str()));
    for space in spaces {
        if !space.is_space() {
            continue;
        }
        let Some(raw) = space.get_state_event(StateEventType::SpaceChild, room_id).await.ok().flatten() else {
            continue;
        };
        let Some(ev) = crate::appstate::raw_to_value(&raw) else { continue };
        let Some(list) = ev.get("content").and_then(|c| c.get("via")).and_then(Value::as_array) else { continue };
        if list.is_empty() {
            continue;
        }
        via.extend(list.iter().filter_map(Value::as_str).map(str::to_string));
        let from = server_part(ev.get("sender").and_then(Value::as_str).unwrap_or_default());
        if !from.is_empty() {
            senders.push(from);
        }
    }
    (via, senders)
}

/// The server of the person who invited me to this room (my own member event's sender), or "".
pub async fn inviter_server_here(room: &Room) -> String {
    let Ok(Some(raw)) = room.get_state_event(StateEventType::RoomMember, room.own_user_id().as_str()).await else {
        return String::new();
    };
    let Some(ev) = crate::appstate::raw_to_value(&raw) else { return String::new() };
    if ev.get("content").and_then(|c| c.get("membership")).and_then(Value::as_str) != Some("invite") {
        return String::new();
    }
    server_part(ev.get("sender").and_then(Value::as_str).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) const VECTORS: &str = include_str!("testvectors/foreign-room.json");

    fn strs(v: &Value, k: &str) -> Vec<String> {
        // non-strings in a list are what the web's pickVia drops; a missing list = none
        v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
    }

    #[test]
    fn vectors() {
        let v: Value = serde_json::from_str(VECTORS).unwrap();
        for c in v["serverPart"].as_array().unwrap() {
            let id = c["id"].as_str().unwrap();
            assert_eq!(server_part(id), c["server"].as_str().unwrap(), "serverPart {id:?}");
        }
        for c in v["origin"].as_array().unwrap() {
            assert_eq!(room_origin(&c["create"]).as_deref(), c["origin"].as_str(), "origin: {}", c["name"]);
        }
        for c in v["managed"].as_array().unwrap() {
            let own = c["own"].as_str().unwrap();
            assert_eq!(is_managed(&c["create"], own), c["managed"].as_bool().unwrap(), "managed: {}", c["name"]);
        }
        for c in v["via"].as_array().unwrap() {
            let (l, ch, h) = (strs(c, "linkVia"), strs(c, "childVia"), strs(c, "hints"));
            let got = pick_via(c["target"].as_str().unwrap(), &refs(&l), &refs(&ch), &refs(&h));
            assert_eq!(got, strs(c, "via"), "via: {}", c["name"]);
        }
        for c in v["plan"].as_array().unwrap() {
            let (types, done) = (strs(c, "types"), strs(c, "done"));
            let pl = Some(&c["pl"]).filter(|p| !p.is_null());
            let got = pl_write_plan(&c["create"], c["own"].as_str().unwrap(), pl, &refs(&types), c["canEdit"].as_bool().unwrap(), &done);
            assert_eq!(got.reason, c["reason"].as_str().unwrap(), "plan: {}", c["name"]);
            assert_eq!(got.types, strs(c, "lower"), "plan: {} (types)", c["name"]);
            if let Some(want) = c.get("events") {
                let content = got.content.clone().expect("a write has content");
                assert_eq!(&content["events"], want, "plan: {} (events)", c["name"]);
                // everything but `events` is kept as it was
                let (mut rest, mut was) = (content.clone(), c["pl"].clone());
                rest.as_object_mut().unwrap().remove("events");
                was.as_object_mut().unwrap().remove("events");
                assert_eq!(rest, was, "plan: {} (rest kept)", c["name"]);
            } else if got.reason != "write" {
                assert!(got.content.is_none(), "plan: {} (no content)", c["name"]);
            }
        }
    }

    fn refs(x: &[String]) -> Vec<&str> {
        x.iter().map(String::as_str).collect()
    }

    /// The PM's acceptance test (2026-10-28): a room on another server gets NO power-level write, on open or on call
    /// join, whatever its power levels and whoever I am there (the web's foreign.test.ts has the same loop).
    #[test]
    fn foreign_room_never_written_on_open_or_call_join() {
        let foreign = [
            json!({"sender": "@friend:friend.example", "content": {"room_version": "10"}}),
            json!({"sender": "@someone:matrix.org", "content": {"room_version": "12"}}),
            json!({"sender": "@owner:home.example", "content": {"room_version": "12", "additional_creators": ["@x:example.org"]}}),
            json!(null),
        ];
        let pls = [json!({"state_default": 50}), json!({"state_default": 100, "events": {}}), json!({}), json!({"events": {"m.call.member": 50}})];
        let on_open = crate::appstate::TINHAT_MEMBER_STATE_EVENTS;
        let call_join = crate::appstate::CALL_MEMBER_STATE_EVENTS;
        let mut writes = 0;
        for create in &foreign {
            for pl in &pls {
                for types in [on_open, call_join] {
                    for can_edit in [true, false] {
                        if pl_write_plan(create, "home.example", Some(pl), types, can_edit, &[]).reason == "write" {
                            writes += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(writes, 0);
        // and the same room on my own server does get one (the loop above isn't vacuous)
        let mine = json!({"sender": "@owner:home.example", "content": {}});
        assert_eq!(pl_write_plan(&mine, "home.example", Some(&pls[0]), call_join, true, &[]).reason, "write");
    }

    #[test]
    fn v12_never_blank() {
        assert!(pick_via("!31hneApxJ_1o-63DmFrpeqnkFfWppnzWso1JvH3ogLM", &[], &[], &[]).is_empty());
        assert!(!pick_via("!31hneApxJ_1o-63DmFrpeqnkFfWppnzWso1JvH3ogLM", &[""], &[""], &[""]).contains(&String::new()));
    }

    #[test]
    fn garbage_never_panics() {
        for v in [json!(null), json!(7), json!("x"), json!({"sender": 7}), json!({"sender": "@a:b.c", "content": 7})] {
            let _ = room_origin(&v);
            let _ = is_managed(&v, "b.c");
        }
        let _ = pick_via("", &["[", "[]", "[::1", "a:", ":"], &[], &[]);
    }

    #[test]
    fn vectors_are_the_webs() {
        let dir = std::env::var_os("TINHAT_WEB_DIR").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("tinhat-web")));
        let Some(dir) = dir else { return };
        let path = dir.join("src/core/matrix/testvectors/foreign-room.json");
        match std::fs::read_to_string(&path) {
            Ok(theirs) => assert!(VECTORS == theirs, "testvectors/foreign-room.json differs from {}", path.display()),
            Err(_) => eprintln!("SKIPPED: no web vectors at {}", path.display()),
        }
    }

    /// The lint (B5 "a test that walks every call site"): a write of `m.room.power_levels` may only appear in the files
    /// that guard it with `managed_here` (appstate.rs ensure_writable, space.rs the roles reconciler) or in room.rs's
    /// person-chosen moderator action (one user's level: plain Matrix moderation). The web's foreign.test.ts has the same.
    #[test]
    fn every_power_level_writer_is_guarded() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let explicit = ["appstate.rs", "space.rs", "room.rs"];
        let mut offenders = Vec::new();
        for e in std::fs::read_dir(&src).unwrap().flatten() {
            let p = e.path();
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            if !name.ends_with(".rs") || name == "foreign_room.rs" {
                continue;
            }
            let text: String = std::fs::read_to_string(&p)
                .unwrap_or_default()
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join(" ");
            // whitespace-insensitive, so a call split over lines is still seen
            let flat: String = text.split_whitespace().collect();
            let writes = flat.contains("send_state_event_raw(\"m.room.power_levels\"")
                || flat.contains("send_power_levels(")
                || flat.contains("update_power_levels(")
                || flat.contains("apply_power_level_changes(")
                || flat.contains("RoomPowerLevelsEventContent") && flat.contains("send_state_event(");
            if writes && !explicit.contains(&name.as_str()) {
                offenders.push(name);
            }
        }
        assert!(offenders.is_empty(), "power-level writers outside the guarded files: {offenders:?}");
        for f in ["appstate.rs", "space.rs"] {
            let text = std::fs::read_to_string(src.join(f)).unwrap();
            assert!(text.contains("managed_here("), "{f} must check managed_here");
        }
        // …and room open goes only through provision_on_open (once per room per session, own server's rooms only)
        let room = std::fs::read_to_string(src.join("room.rs")).unwrap();
        assert!(room.contains("provision_on_open(") && !room.contains("provision_room(") && !room.contains("ensure_writable("));
        let app = std::fs::read_to_string(src.join("appstate.rs")).unwrap();
        assert!(app.contains("pl_write_plan(") && app.contains("send_power_levels(room, content)"));
        assert_eq!(app.matches("send_state_event_raw(\"m.room.power_levels\"").count(), 1, "one place sends power levels");
    }
}
