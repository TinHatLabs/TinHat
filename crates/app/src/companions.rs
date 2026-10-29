//! Screen share P3, the receivers' half (share `claude/tinhat-android/SCREEN-SHARE.md` §4): the Android app shares
//! the screen from its NATIVE device, a second call participant of the same person whose `dev.tinhat.client` says
//! `"role": "share", "companion_of": "<the person's main call device>"`. Receivers show that share as the person's own:
//! no second avatar tile, no second name, the share tile right after the person's tile(s).
//!
//! Pure. = the web's `src/features/calls/companions.ts` (`companions` + `foldCompanions`) and the mobile core's
//! `rtc::companions`: the same rule and the same vectors (`testvectors/companions.json`, owned by the web, copied byte
//! for byte). Wire format unchanged.

use serde_json::Value;

/// The self tile's participant: a companion of OUR OWN device folds into it.
pub const SELF_PARTICIPANT: &str = "You";

/// `(companion identity, main identity)` (`@user:server:DEVICE`), sorted, deduplicated: every membership event whose
/// content has `dev.tinhat.client.role == "share"` and a string `companion_of` other than its own `device_id`, where
/// the companion AND the named main are live sessions of the SAME sender. Orphans and claims on someone else's device
/// are left out (they show as ordinary participants). On top of the mobile core's rule (the review hardenings, in the
/// shared vectors): the marker must come from the companion's live session's OWN event (its state key), a companion
/// naming two mains is none, and a main that is itself a companion folds nothing (no chains, no cycles). `live` =
/// `(sender, device_id, state_key)` of the live sessions.
pub fn companions(events: &[Value], live: &[(String, String, String)]) -> Vec<(String, String)> {
    let session = |user: &str, device: &str| live.iter().find(|(u, d, _)| u == user && d == device);
    let mut mains: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> = std::collections::BTreeMap::new();
    for ev in events {
        let Some(sender) = ev.get("sender").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) else { continue };
        let Some(content) = ev.get("content") else { continue };
        let Some(device) = content.get("device_id").and_then(|v| v.as_str()) else { continue };
        let Some(client) = content.get("dev.tinhat.client") else { continue };
        if client.get("role").and_then(|v| v.as_str()) != Some("share") {
            continue;
        }
        let Some(main) = client.get("companion_of").and_then(|v| v.as_str()) else { continue };
        if main == device {
            continue;
        }
        // a missing / non-string state key is no event of a live session (the web's `stateKey !== ev.state_key`)
        let Some(state_key) = ev.get("state_key").and_then(|v| v.as_str()) else { continue };
        let own_ok = session(sender, device).is_some_and(|(_, _, sk)| sk == state_key);
        if !own_ok || session(sender, main).is_none() {
            continue;
        }
        mains.entry(format!("{sender}:{device}")).or_default().insert(format!("{sender}:{main}"));
    }
    let mut out: Vec<(String, String)> = mains
        .iter()
        .filter(|(_, set)| set.len() == 1) // ambiguous: none
        .map(|(comp, set)| (comp.clone(), set.iter().next().cloned().unwrap_or_default()))
        .filter(|(_, main)| !mains.contains_key(main)) // the main is itself a companion: a chain or a cycle
        .collect();
    out.sort();
    out
}

fn str_of<'a>(t: &'a Value, k: &str) -> &'a str {
    t.get(k).and_then(|v| v.as_str()).unwrap_or("")
}

/// Fold companions into their person on the call stage (`tiles` = the JSON objects `tiles_json_enriched` builds:
/// self first, then video tiles, then avatar tiles of people without a camera). A companion's avatar tile goes; its
/// screen tile keeps its `participant` (the track lives there), gains `companionOf` = the main participant ("You" for
/// our own device) + the main's `name`, and moves right after the main's last tile. A companion whose main has no tile
/// is left as it was.
pub fn fold_tiles(tiles: Vec<Value>, pairs: &[(String, String)], own_identity: &str) -> Vec<Value> {
    if pairs.is_empty() {
        return tiles;
    }
    let main_of = |participant: &str| -> Option<String> {
        pairs.iter().find(|(c, _)| c == participant).map(|(_, m)| if m == own_identity { SELF_PARTICIPANT.to_string() } else { m.clone() })
    };
    let name_of = |main: &str| -> Option<String> {
        tiles.iter().find(|t| str_of(t, "participant") == main && t.get("companionOf").is_none()).map(|t| str_of(t, "name").to_string())
    };
    let mut kept: Vec<Value> = Vec::new();
    let mut moved: Vec<Value> = Vec::new();
    for t in &tiles {
        let main = main_of(str_of(t, "participant"));
        let name = main.as_deref().and_then(name_of);
        let (Some(main), Some(name)) = (main, name) else {
            kept.push(t.clone());
            continue;
        };
        if str_of(t, "kind") == "avatar" {
            continue; // the person is on the stage already
        }
        let share = str_of(t, "kind") == "screenshare" || t.get("screenshare").and_then(|v| v.as_bool()) == Some(true);
        let mut t = t.clone();
        if share {
            if let Some(o) = t.as_object_mut() {
                o.insert("companionOf".into(), Value::String(main));
                o.insert("name".into(), Value::String(name));
            }
        }
        moved.push(t);
    }
    for t in moved {
        let Some(main) = t.get("companionOf").and_then(|v| v.as_str()).map(str::to_string) else {
            kept.push(t);
            continue;
        };
        let at = kept
            .iter()
            .rposition(|k| str_of(k, "participant") == main || k.get("companionOf").and_then(|v| v.as_str()) == Some(main.as_str()))
            .map(|i| i + 1)
            .unwrap_or(0);
        kept.insert(at, t);
    }
    kept
}

/// Call stats (screen share P3 follow-up): fold a companion's row into its person's row — one row per person, the
/// share's numbers as the row's `share`. `peers` = the stats snapshot's rows (objects with an `id` = LiveKit identity),
/// in order; one share per main device (the apps start one per page: a second companion of the same main stays a row);
/// a companion whose main has no row stays a row; a companion of our own device stays a row with
/// `ownShare: true` ("Your screen"). = the web's companions.ts `foldPeers` (same cases).
pub fn fold_peers(peers: Vec<Value>, pairs: &[(String, String)], own_identity: &str) -> Vec<Value> {
    if pairs.is_empty() {
        return peers;
    }
    let main_of = |id: &str| pairs.iter().find(|(c, _)| c == id).map(|(_, m)| m.clone());
    let ids: Vec<String> = peers.iter().map(|p| str_of(p, "id").to_string()).collect();
    let mut share_of: Vec<(String, Value)> = Vec::new(); // main → its companion's row (the first)
    for p in &peers {
        if let Some(main) = main_of(str_of(p, "id")) {
            if main != own_identity && ids.contains(&main) && !share_of.iter().any(|(m, _)| *m == main) {
                share_of.push((main, p.clone()));
            }
        }
    }
    let folded: Vec<String> = share_of.iter().map(|(_, v)| str_of(v, "id").to_string()).collect();
    let mut out = Vec::new();
    for p in peers {
        let id = str_of(&p, "id").to_string();
        if folded.contains(&id) {
            continue;
        }
        let mut row = p;
        if let Some(o) = row.as_object_mut() {
            if let Some((_, sh)) = share_of.iter().find(|(m, _)| *m == id) {
                o.insert("share".into(), sh.clone());
            }
            if main_of(&id).as_deref() == Some(own_identity) {
                o.insert("ownShare".into(), Value::Bool(true));
            }
        }
        out.push(row);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) const VECTORS: &str = include_str!("testvectors/companions.json");

    fn pair(v: &Value) -> (String, String) {
        (v[0].as_str().unwrap().to_string(), v[1].as_str().unwrap().to_string())
    }

    #[test]
    fn vectors() {
        let v: Value = serde_json::from_str(VECTORS).unwrap();
        let cases = v["cases"].as_array().unwrap();
        assert!(cases.len() >= 12);
        for c in cases {
            let events: Vec<Value> = c["events"].as_array().unwrap().clone();
            let live: Vec<(String, String, String)> = c["live"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| (l[0].as_str().unwrap().to_string(), l[1].as_str().unwrap().to_string(), l[2].as_str().unwrap().to_string()))
                .collect();
            let want: Vec<(String, String)> = c["want"].as_array().unwrap().iter().map(pair).collect();
            assert_eq!(companions(&events, &live), want, "{}", c["name"]);
        }
    }

    /// The byte copy equals the web's (`TINHAT_WEB_DIR` = a tinhat-web tree, else `$HOME/tinhat-web`; skipped when
    /// neither has the file).
    /// = the web (and the mobile core 6aa95e9): a live session with an empty state key still needs the event's own
    /// string state key, and an empty sender is no sender.
    #[test]
    fn state_key_and_sender_must_be_strings() {
        let live = |u: &str, d: &str, sk: &str| (u.to_string(), d.to_string(), sk.to_string());
        let l = vec![live("@sam:x", "MAIN", "_@sam:x_MAIN"), live("@sam:x", "P", "")];
        let share = |sk: Value| serde_json::json!({"sender": "@sam:x", "state_key": sk, "content": {"device_id": "P",
            "dev.tinhat.client": {"role": "share", "companion_of": "MAIN"}}});
        for sk in [Value::Null, serde_json::json!(3)] {
            assert!(companions(&[share(sk)], &l).is_empty());
        }
        let mut no_key = share(serde_json::json!(""));
        no_key.as_object_mut().unwrap().remove("state_key");
        assert!(companions(&[no_key], &l).is_empty());
        assert_eq!(companions(&[share(serde_json::json!(""))], &l), vec![("@sam:x:P".to_string(), "@sam:x:MAIN".to_string())]);
        let anon = vec![live("", "MAIN", "__MAIN"), live("", "P", "__P")];
        let ev = serde_json::json!({"sender": "", "state_key": "__P", "content": {"device_id": "P",
            "dev.tinhat.client": {"role": "share", "companion_of": "MAIN"}}});
        assert!(companions(&[ev], &anon).is_empty());
    }

    #[test]
    fn vectors_are_the_webs() {
        let dir = std::env::var_os("TINHAT_WEB_DIR").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("tinhat-web")));
        let Some(dir) = dir else { return };
        let path = dir.join("src/features/calls/testvectors/companions.json");
        match std::fs::read_to_string(&path) {
            Ok(theirs) => assert!(VECTORS == theirs, "testvectors/companions.json differs from {}", path.display()),
            Err(_) => eprintln!("SKIPPED: no web vectors at {}", path.display()),
        }
    }

    #[test]
    fn garbage_never_panics() {
        let events = vec![json!(null), json!({}), json!({"sender": 7}), json!({"sender": "@a:x", "content": "x"})];
        assert!(companions(&events, &[]).is_empty());
    }

    fn tile(id: &str, participant: &str, kind: &str, name: &str) -> Value {
        json!({"id": id, "participant": participant, "kind": kind, "name": name, "screenshare": kind == "screenshare",
               "muted": false, "avatar": "", "serverMuted": false, "serverMutedBy": ""})
    }
    fn ids(t: &[Value]) -> Vec<&str> {
        t.iter().map(|x| str_of(x, "id")).collect()
    }
    const ME: &str = "@me:x:MYDEV";
    fn base() -> Vec<Value> {
        vec![
            tile("vid_selfview", "You", "video", "You"),
            tile("vid_cam1", "@sam:x:MAIN", "video", "Sam"),
            tile("vid_ss9", "@sam:x:PHONE", "screenshare", "Sam"),
            tile("vid_cam2", "@amy:x:A1", "video", "Amy"),
            tile("avatar_@sam:x:PHONE", "@sam:x:PHONE", "avatar", "Sam"),
            tile("avatar_@bob:x:B1", "@bob:x:B1", "avatar", "Bob"),
        ]
    }
    fn p(c: &str, m: &str) -> Vec<(String, String)> {
        vec![(c.to_string(), m.to_string())]
    }

    // = the web's companions.test.ts "the stage fold" cases
    #[test]
    fn fold_puts_the_share_with_its_person() {
        let out = fold_tiles(base(), &p("@sam:x:PHONE", "@sam:x:MAIN"), ME);
        assert_eq!(ids(&out), ["vid_selfview", "vid_cam1", "vid_ss9", "vid_cam2", "avatar_@bob:x:B1"]);
        let ss = out.iter().find(|t| str_of(t, "id") == "vid_ss9").unwrap();
        assert_eq!(str_of(ss, "participant"), "@sam:x:PHONE");
        assert_eq!(str_of(ss, "companionOf"), "@sam:x:MAIN");
        assert_eq!(str_of(ss, "name"), "Sam");
    }

    #[test]
    fn fold_person_without_a_camera() {
        let t = vec![
                tile("vid_selfview", "You", "video", "You"),
                tile("vid_ss9", "@sam:x:PHONE", "screenshare", "Sam"),
                tile("avatar_@sam:x:MAIN", "@sam:x:MAIN", "avatar", "Sam"),
                tile("avatar_@sam:x:PHONE", "@sam:x:PHONE", "avatar", "Sam"),
            ];
        let out = fold_tiles(t, &p("@sam:x:PHONE", "@sam:x:MAIN"), ME);
        assert_eq!(ids(&out), ["vid_selfview", "avatar_@sam:x:MAIN", "vid_ss9"]);
    }

    #[test]
    fn fold_own_companion_into_you() {
        let t = vec![
                tile("vid_selfview", "You", "video", "You"),
                tile("vid_ss1", "@me:x:PHONE", "screenshare", "Me"),
                tile("avatar_@me:x:PHONE", "@me:x:PHONE", "avatar", "Me"),
                tile("avatar_@bob:x:B1", "@bob:x:B1", "avatar", "Bob"),
            ];
        let out = fold_tiles(t, &p("@me:x:PHONE", ME), ME);
        assert_eq!(ids(&out), ["vid_selfview", "vid_ss1", "avatar_@bob:x:B1"]);
        assert_eq!(str_of(&out[1], "companionOf"), "You");
        assert_eq!(str_of(&out[1], "name"), "You");
    }

    #[test]
    fn fold_leaves_orphans_and_empty_alone() {
        assert_eq!(fold_tiles(base(), &p("@sam:x:PHONE", "@sam:x:GONE"), ME), base());
        assert_eq!(fold_tiles(base(), &[], ME), base());
    }

    #[test]
    fn fold_two_tracks_of_one_companion_stay_in_order() {
        let mut t = base();
        t.push(tile("vid_ss10", "@sam:x:PHONE", "screenshare", "Sam"));
        let out = fold_tiles(t, &p("@sam:x:PHONE", "@sam:x:MAIN"), ME);
        assert_eq!(ids(&out), ["vid_selfview", "vid_cam1", "vid_ss9", "vid_ss10", "vid_cam2", "avatar_@bob:x:B1"]);
    }

    // = the web's companions.test.ts "the Call stats rows" cases (pairs from the shared vectors' first case)
    #[test]
    fn fold_peers_cases() {
        let v: Value = serde_json::from_str(VECTORS).unwrap();
        let c0 = &v["cases"][0];
        let events: Vec<Value> = c0["events"].as_array().unwrap().clone();
        let live: Vec<(String, String, String)> = c0["live"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| (l[0].as_str().unwrap().to_string(), l[1].as_str().unwrap().to_string(), l[2].as_str().unwrap().to_string()))
            .collect();
        let pairs = companions(&events, &live);
        let peer = |id: &str| json!({"id": id, "kbps": id.len()});
        let ids = |l: &[Value]| -> Vec<String> {
            l.iter()
                .map(|x| {
                    let mut s = str_of(x, "id").to_string();
                    if let Some(sh) = x.get("share") {
                        s.push_str(&format!("+{}", str_of(sh, "id")));
                    }
                    if x.get("ownShare") == Some(&Value::Bool(true)) {
                        s.push_str("!own");
                    }
                    s
                })
                .collect()
        };
        let p = vec![peer("@sam:x:MAIN"), peer("@sam:x:PHONE"), peer("@eve:x:EVIL"), peer("@bob:x:B1")];
        assert_eq!(ids(&fold_peers(p.clone(), &pairs, ME)), ["@sam:x:MAIN+@sam:x:PHONE", "@eve:x:EVIL", "@bob:x:B1"]);
        assert_eq!(ids(&fold_peers(vec![p[1].clone(), p[3].clone(), p[0].clone()], &pairs, ME)), ["@bob:x:B1", "@sam:x:MAIN+@sam:x:PHONE"]);
        assert_eq!(ids(&fold_peers(vec![p[1].clone(), p[3].clone()], &pairs, ME)), ["@sam:x:PHONE", "@bob:x:B1"]);
        let own = vec![("@me:x:PHONE".to_string(), ME.to_string())];
        assert_eq!(ids(&fold_peers(vec![peer("@me:x:PHONE"), p[3].clone()], &own, ME)), ["@me:x:PHONE!own", "@bob:x:B1"]);
        assert_eq!(ids(&fold_peers(p.clone(), &[], ME)), ["@sam:x:MAIN", "@sam:x:PHONE", "@eve:x:EVIL", "@bob:x:B1"]);
        let f = fold_peers(p, &pairs, ME);
        assert_eq!(f[0]["share"]["kbps"], json!("@sam:x:PHONE".len()));
    }
}
