//! The reference's hand-written tests (`src/actions/actions.test.ts`), mirrored test by test: the worked examples of
//! ACTIONS.md §13, the table's own invariants (the staged default rows placeable, every menu item fitting its menu),
//! the per-app summary, and robustness — no entry point panics, whatever it is given (the reference's seeded run, plus
//! a larger structural fuzz over random and mutated inputs and raw JSON text).

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};
use tinhat_actions::{call, dispatch, drawer, evaluate, menu, palette, place, row, summary, table, KINDS};
use tinhat_theme::canon::canon_json;
use tinhat_theme::json::parse_json;

const DEFAULT_ROWS: &str = include_str!("../default-rows.json");

// ---- ACTIONS.md §13: the worked examples ----------------------------------------------------------------------------

const ROOM: &str = "!room:example.com";

fn world(values: Value, room: Value) -> Value {
    json!({
        "values": values,
        "current": {"room": ROOM, "activeSpace": "!space:example.com"},
        "subjects": {"room": {ROOM: {"name": "Gaming Central", "values": room}}},
    })
}

fn w0() -> Value {
    world(json!({}), json!({}))
}

fn strs(v: &[&Value]) -> Vec<Value> {
    v.iter().map(|x| (*x).clone()).collect()
}

#[test]
fn example_1_mute_a_toggle_with_a_face_per_state() {
    let off = evaluate(&json!({"app": "desktop", "action": "call.mic.toggle", "snapshot": world(json!({"call.in": true}), json!({}))}));
    assert_eq!(off["enabled"], true);
    assert_eq!(off["active"], false);
    assert_eq!(off["label"], "Mute");
    let on = evaluate(&json!({"app": "web", "action": "call.mic.toggle", "snapshot": world(json!({"call.in": true, "call.micMuted": true}), json!({}))}));
    assert_eq!(on["active"], true);
    assert_eq!(on["label"], "Unmute");
    assert_eq!(on["icon"], "microphone-sensitivity-muted-symbolic");
    let out = evaluate(&json!({"app": "desktop", "action": "call.mic.toggle", "snapshot": world(json!({}), json!({}))}));
    assert_eq!(strs(&[&out["visible"], &out["enabled"], &out["why"]]), vec![json!(true), json!(false), json!("when")]);
}

#[test]
fn example_2_join_a_rooms_call_variants_a_stored_target_an_unknown_room_dims() {
    let live = evaluate(&json!({"app": "web", "action": "call.join", "snapshot": world(json!({}), json!({"room.callLive": true}))}));
    assert_eq!(live["label"], "Join the active call");
    assert_eq!(live["targetName"], "Gaming Central");
    let gone = evaluate(&json!({"app": "web", "action": "call.join", "target": {"room": "!gone:example.com"}, "snapshot": w0()}));
    assert_eq!(
        strs(&[&gone["visible"], &gone["enabled"], &gone["why"], &gone["whyTarget"]]),
        vec![json!(true), json!(false), json!("target"), json!("target-unknown")]
    );
    // the header hides it in a call (the row's `requires`) and dims it while connecting (the action's `when`)
    let in_row = evaluate(&json!({"app": "desktop", "action": "call.join", "row": "header.main", "snapshot": world(json!({"call.in": true}), json!({}))}));
    assert_eq!(strs(&[&in_row["visible"], &in_row["enabled"], &in_row["why"]]), vec![json!(false), json!(false), json!("requires")]);
    let conn = evaluate(&json!({"app": "desktop", "action": "call.join", "row": "header.main", "snapshot": world(json!({"call.connecting": true}), json!({}))}));
    assert_eq!(strs(&[&conn["visible"], &conn["enabled"], &conn["why"]]), vec![json!(true), json!(false), json!("when")]);
}

#[test]
fn example_3_leave_room_danger_confirm_with_the_rooms_name_the_invite_variant() {
    let p = evaluate(&json!({"app": "desktop", "action": "room.leave", "snapshot": world(json!({}), json!({"room.joined": true}))}));
    assert_eq!(p["danger"], true);
    assert_eq!(p["confirm"]["title"], "Leave room?");
    assert_eq!(p["confirm"]["text"], "Leave \"Gaming Central\"? You'll need a new invite to rejoin if it's private.");
    let inv = evaluate(&json!({"app": "desktop", "action": "room.leave", "snapshot": world(json!({}), json!({"room.invited": true}))}));
    assert_eq!(inv["label"], "Decline invite");
    assert_eq!(inv["confirm"], Value::Null);
}

#[test]
fn example_4_rail_buttons_a_listed_target_value_names_and_draws_the_button() {
    let home = evaluate(&json!({"app": "desktop", "action": "nav.space.select", "target": {"rail": "home"}, "snapshot": w0()}));
    assert_eq!(
        strs(&[&home["label"], &home["icon"], &home["targetName"], &home["subject"]]),
        vec![json!("Home"), json!("go-home-symbolic"), json!("Home"), Value::Null]
    );
    let pick = evaluate(&json!({"app": "desktop", "action": "nav.space.select", "snapshot": w0()}));
    assert_eq!(strs(&[&pick["enabled"], &pick["pick"]]), vec![json!(true), json!(true)]);
}

#[test]
fn example_5_set_a_status_consent_only_while_sharing_is_off_the_presets_label() {
    let lfg = evaluate(&json!({"app": "web", "action": "status.set", "target": {"status": "lfg"}, "snapshot": world(json!({"presence.known": true}), json!({}))}));
    assert_eq!(lfg["label"], "Looking for group");
    assert_eq!(lfg["emoji"], "🎯");
    assert_eq!(lfg["confirm"]["kind"], "consent");
    let sharing = evaluate(&json!({
        "app": "web", "action": "status.set", "target": {"status": "lfg"},
        "snapshot": world(json!({"presence.known": true, "presence.sharing": true}), json!({})),
    }));
    assert_eq!(sharing["confirm"], Value::Null);
}

#[test]
fn example_6_a_device_local_target_resolves_through_this_devices_table() {
    // `{ ...world(), local: …, subjects: … }`: the spread's subjects are replaced
    let mut snap = w0();
    snap["local"] = json!({"h1": {"device": "alsa_input.headset", "label": "Headset"}});
    snap["subjects"] = json!({"device": {"alsa_input.headset": {"values": {}}}});
    let ok = evaluate(&json!({"app": "desktop", "action": "call.input.set", "target": {"local": "h1"}, "snapshot": snap}));
    assert_eq!(strs(&[&ok["enabled"], &ok["subject"], &ok["targetName"]]), vec![json!(true), json!("alsa_input.headset"), json!("Headset")]);
    let elsewhere = evaluate(&json!({"app": "desktop", "action": "call.input.set", "target": {"local": "h1"}, "snapshot": w0()}));
    assert_eq!(strs(&[&elsewhere["enabled"], &elsewhere["pick"]]), vec![json!(true), json!(true)]);
    let raw = evaluate(&json!({"app": "desktop", "action": "call.input.set", "target": {"device": "alsa_input.headset"}, "snapshot": snap}));
    assert_eq!(raw["whyTarget"], "target-invalid:not-portable");
}

#[test]
fn example_7_platform_web_comes_from_the_app_never_the_snapshot() {
    let w = evaluate(&json!({"app": "web", "action": "app.logship.toggle", "snapshot": world(json!({"platform.web": false}), json!({}))}));
    assert_eq!(strs(&[&w["enabled"], &w["why"]]), vec![json!(false), json!("when")]);
    let d = evaluate(&json!({"app": "desktop", "action": "app.logship.toggle", "snapshot": world(json!({"platform.web": true}), json!({}))}));
    assert_eq!(d["enabled"], true);
}

#[test]
fn example_8_a_newer_tin_hats_button_kept_drawn_as_the_placeholder_hidden_with_hide_when_off() {
    let r = row(&json!({"app": "desktop", "row": "buttons.main", "value": {"items": [{"a": "call.teleport"}, {"a": "call.teleport", "hideWhenOff": true}, {"a": "Not An Id"}]}}));
    let got: Vec<Value> = r["items"].as_array().unwrap().iter().map(|i| json!([i["a"], i["known"], i["state"], i["why"]])).collect();
    assert_eq!(
        got,
        vec![
            json!(["call.teleport", false, "dimmed", "unknown"]),
            json!(["call.teleport", false, "hidden", "unknown"]),
            json!(["Not An Id", false, "hidden", "bad"]),
        ]
    );
    assert_eq!(r["items"][0]["label"], table().placeholder.label.as_str());
    assert_eq!(r["items"][0]["icon"], table().placeholder.icon.as_str());
    assert_eq!(strs(&[&r["look"], &r["scale"], &r["style"], &r["editable"]]), vec![json!("iconLabel"), json!("M"), json!("tool"), json!(true)]);
}

#[test]
fn example_9_the_palette_rooms_first_an_action_by_its_label_the_room_you_are_in_last() {
    let snap = json!({
        "values": {"call.in": true},
        "current": {"room": "!b:x"},
        "subjects": {"room": {"!a:x": {"name": "Mute city", "values": {"room.joined": true}}, "!b:x": {"name": "Muted", "values": {"room.joined": true}}}},
        "lists": {"room": ["!b:x", "!a:x"]},
    });
    let empty = palette(&json!({"app": "desktop", "snapshot": snap, "query": ""}));
    let ids: Vec<&str> = empty["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["!a:x", "!b:x"]);
    let mute = palette(&json!({"app": "desktop", "snapshot": snap, "query": "mute"}));
    let items = mute["items"].as_array().unwrap();
    let first2: Vec<Value> = items.iter().take(2).map(|i| json!([i["kind"], i["id"]])).collect();
    assert_eq!(first2, vec![json!(["room", "!a:x"]), json!(["action", "call.mic.toggle"])]);
    assert_eq!(items[1]["hint"], "Ctrl+Shift+M");
    let first_last = items.iter().position(|i| i["id"] == "!b:x").expect("the room you are in is listed");
    assert!(items[..first_last].iter().all(|i| i["enabled"] == true), "the room you are in sorts after every available match");
    let actions_only = palette(&json!({"app": "desktop", "snapshot": snap, "query": ">mute"}));
    assert_eq!(actions_only["mode"], "action");
    assert!(actions_only["items"].as_array().unwrap().iter().all(|i| i["kind"] == "action"));
    let theme = palette(&json!({
        "app": "web",
        "snapshot": {"subjects": {"theme": {"oled": {"name": "OLED"}, "goose": {"name": "Goose"}}}, "values": {"prefs.theme": "goose"}},
        "query": "go",
        "stage": "app.theme.set",
    }));
    let got: Vec<Value> = theme["items"].as_array().unwrap().iter().map(|i| json!([i["kind"], i["id"], i["a"], i["target"]])).collect();
    assert_eq!(got, vec![json!(["choice", "goose", "app.theme.set", {"theme": "goose"}])]);
}

#[test]
fn example_12_ctrl_k_lists_pending_invites_newest_first_ranked_like_rooms_never_a_space_no_action() {
    let snap = json!({
        "current": {"room": "!cur:x"},
        "subjects": {
            "room": {
                "!cur:x": {"name": "Lobby", "values": {"room.joined": true}},
                "!zane:x": {"name": "Zane", "values": {"room.joined": true, "room.isDm": true}},
                "!movie:x": {"name": "Movie night", "values": {"room.joined": true}},
                "!invZane:x": {"name": "Zane", "values": {"room.invited": true, "room.isDm": true}},
                "!invMovie:x": {"name": "Movie night 2", "values": {"room.invited": true}},
                "!both:x": {"name": "Joined anyway", "values": {"room.invited": true, "room.joined": true}},
            },
            "space": {"!sp:x": {"name": "Zane's space", "values": {"space.invited": true}}},
        },
        "lists": {
            "room": ["!invZane:x", "!invMovie:x", "!cur:x", "!zane:x", "!movie:x", "!both:x"],
            "invite": ["!invMovie:x", "!invZane:x", "!both:x"],
        },
    });
    let rows = |o: &Value| -> Vec<Value> { o["items"].as_array().unwrap().iter().map(|i| json!([i["kind"], i["id"]])).collect() };
    // empty query: the invites in lists.invite order (newest first), then the rooms, the room you are in last
    let empty = palette(&json!({"app": "desktop", "snapshot": snap, "query": ""}));
    assert_eq!(
        rows(&empty),
        vec![
            json!(["invite", "!invMovie:x"]),
            json!(["invite", "!invZane:x"]),
            json!(["room", "!zane:x"]),
            json!(["room", "!movie:x"]),
            json!(["room", "!both:x"]),
            json!(["room", "!cur:x"]),
        ]
    );
    // the same name and score: the invite first (kind order); a space invite never
    let zane: Vec<Value> = rows(&palette(&json!({"app": "web", "snapshot": snap, "query": "zane"}))).into_iter().filter(|r| r[0] != "action").collect();
    assert_eq!(zane, vec![json!(["invite", "!invZane:x"]), json!(["room", "!zane:x"])]);
    let movie = rows(&palette(&json!({"app": "web", "snapshot": snap, "query": "movie"})));
    assert_eq!(movie[..2].to_vec(), vec![json!(["invite", "!invMovie:x"]), json!(["room", "!movie:x"])]);
    // # = rooms and invites; @ and > = none
    let hash = palette(&json!({"app": "desktop", "snapshot": snap, "query": "#zane"}));
    assert_eq!(hash["mode"], "room");
    assert_eq!(rows(&hash), vec![json!(["invite", "!invZane:x"]), json!(["room", "!zane:x"])]);
    let at = palette(&json!({"app": "desktop", "snapshot": snap, "query": "@zane"}));
    assert!(at["items"].as_array().unwrap().iter().all(|i| i["kind"] != "invite"));
    let act = palette(&json!({"app": "desktop", "snapshot": snap, "query": ">zane"}));
    assert!(act["items"].as_array().unwrap().iter().all(|i| i["kind"] == "action"));
    // an invite row runs no action: the host opens the invite (ACTIONS.md §14.3)
    let it = &palette(&json!({"app": "web", "snapshot": snap, "query": ""}))["items"][0];
    let got = json!([it["a"], it["target"], it["hint"], it["icon"], it["active"], it["enabled"], it["pick"]]);
    assert_eq!(got, json!([null, null, null, null, null, true, false]));
    // the table's marker and kind order
    assert_eq!(table().palette.texts["inviteMarker"], "Invited");
    assert_eq!(table().palette.kind_order[..2].to_vec(), vec!["invite".to_string(), "room".to_string()]);
}

#[test]
fn example_11_a_stored_target_names_its_button_target_face_the_absent_rule_does_not() {
    let snap = json!({
        "current": {"room": ROOM},
        "subjects": {
            "room": {ROOM: {"name": "Gaming Central", "values": {}}, "!v:x": {"name": "Valheim", "values": {"room.callLive": true}}},
            "theme": {"goose": {"name": "Goose"}},
        },
    });
    let stored = evaluate(&json!({"app": "web", "action": "call.join", "target": {"room": "!v:x"}, "snapshot": snap}));
    assert_eq!(strs(&[&stored["label"], &stored["shortLabel"]]), vec![json!("Join Valheim"), json!("Valheim")]);
    let current = evaluate(&json!({"app": "web", "action": "call.join", "snapshot": snap}));
    assert_eq!(current["label"], "Start a call");
    let theme = evaluate(&json!({"app": "desktop", "action": "app.theme.set", "target": {"theme": "goose"}, "snapshot": snap}));
    assert_eq!(strs(&[&theme["label"], &theme["tip"]]), vec![json!("Goose"), json!("Switch to the Goose theme")]);
}

#[test]
fn example_10_the_drawer_of_your_own_button_island_has_no_context_only_actions() {
    let d = drawer(&json!({"app": "web", "row": "buttons.main", "snapshot": {}}));
    let ids: Vec<String> = d["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["items"].as_array().unwrap().iter().map(|i| i["a"].as_str().unwrap().to_string()))
        .collect();
    for id in &ids {
        let a = table().actions.iter().find(|a| &a.id == id).expect("a table action");
        assert!(a.contexts.is_none(), "{id}");
    }
    assert!(ids.iter().any(|i| i == "call.mic.toggle"));
    assert!(!ids.iter().any(|i| i == "message.reply"));
}

#[test]
fn example_13_keep_call_buttons_visible_a_preference_toggle_short_faces_for_the_bars_pin_never_on_phones() {
    let off = evaluate(&json!({"app": "desktop", "action": "call.bar.pin.toggle", "snapshot": w0()}));
    assert_eq!(
        strs(&[&off["visible"], &off["enabled"], &off["active"], &off["label"], &off["shortLabel"], &off["icon"]]),
        vec![json!(true), json!(true), json!(false), json!("Keep call buttons visible"), json!("Keep buttons visible"), json!("window-pin-symbolic")]
    );
    let on = evaluate(&json!({"app": "web", "action": "call.bar.pin.toggle", "snapshot": world(json!({"call.in": true, "prefs.callBarPinned": true}), json!({}))}));
    assert_eq!(
        strs(&[&on["enabled"], &on["active"], &on["label"], &on["shortLabel"]]),
        vec![json!(true), json!(true), json!("Let call buttons hide"), json!("Let buttons hide")]
    );
    let phone = evaluate(&json!({"app": "web", "action": "call.bar.pin.toggle", "snapshot": world(json!({"ui.phone": true}), json!({}))}));
    assert_eq!(strs(&[&phone["visible"], &phone["enabled"], &phone["why"]]), vec![json!(false), json!(false), json!("requires")]);
    // Ctrl+K: "auto hide" finds it by its keyword (no default key); never on a phone
    let p = palette(&json!({"app": "desktop", "snapshot": w0(), "query": "auto hide"}));
    let first = &p["items"][0];
    assert_eq!(
        json!([first["kind"], first["id"], first["label"], first["hint"], first["active"]]),
        json!(["action", "call.bar.pin.toggle", "Keep call buttons visible", null, false])
    );
    let ph = palette(&json!({"app": "web", "snapshot": world(json!({"ui.phone": true}), json!({})), "query": "auto hide"}));
    assert!(!ph["items"].as_array().unwrap().iter().any(|i| i["id"] == "call.bar.pin.toggle"));
    // assignable: Settings → Shortcuts lists it under Calls, with no key until someone gives it one
    let sc = dispatch("shortcuts", &json!({"app": "web"})).expect("the shortcuts view");
    let calls = sc["groups"].as_array().unwrap().iter().find(|g| g["group"] == "calls").expect("the Calls group");
    let it = calls["items"].as_array().unwrap().iter().find(|i| i["a"] == "call.bar.pin.toggle").expect("the action is listed");
    assert_eq!(json!([it["label"], it["keys"], it["defaults"]]), json!(["Keep call buttons visible", [], []]));
}

// ---- the table's own rules ------------------------------------------------------------------------------------------

#[test]
fn table_every_staged_default_row_item_fits_its_row_every_menu_item_its_menu() {
    let d = parse_json(DEFAULT_ROWS).expect("default-rows.json is JSON");
    let mut n = 0;
    for (preset, forms) in d["presets"].as_object().expect("presets") {
        for (form, f) in forms.as_object().expect("forms") {
            for (isl, o) in f["opts"].as_object().expect("opts") {
                for (rn, r) in o["rows"].as_object().expect("rows") {
                    let rk = format!("{isl}.{rn}");
                    let kind = table().rows.get(&rk).unwrap_or_else(|| panic!("{preset}/{form}: unknown row {rk}"));
                    assert!(kind.forms.contains(form), "{preset}/{form}: {rk} is not a {form} row");
                    for it in r["items"].as_array().expect("items") {
                        assert_eq!(place(&json!({"row": rk, "item": it}))["ok"], true, "{preset}/{form} {rk}: {}", canon_json(it));
                        n += 1;
                    }
                }
            }
        }
    }
    assert!(n > 100, "only {n} default row items");
    for (mid, m) in &table().menus {
        let r = menu(&json!({"app": "desktop", "menu": mid, "form": "desktop", "snapshot": {}}));
        let items = r["items"].as_array().expect("menu items");
        assert_eq!(items.len(), m.items.len(), "{mid}");
        for it in items {
            if it.get("a").is_some() {
                assert_ne!(it["why"], "context", "{mid}: {} cannot sit in this menu", it["a"]);
            }
        }
    }
}

#[test]
fn summary_the_per_app_view_lists_exactly_the_apps_actions() {
    for app in ["desktop", "web"] {
        let s = summary(&json!({"app": app}));
        let ids: Vec<&str> = s["actions"].as_array().unwrap().iter().map(|a| a["id"].as_str().unwrap()).collect();
        let want: Vec<&str> = table().actions.iter().filter(|a| a.apps.iter().any(|x| x == app)).map(|a| a.id.as_str()).collect();
        assert_eq!(ids, want, "{app}");
    }
}

#[test]
fn the_typed_table_holds_every_action_and_registry_of_the_json() {
    let t = table();
    let j = &t.json;
    assert_eq!(t.actions.len(), j["actions"].as_array().unwrap().len());
    for (a, raw) in t.actions.iter().zip(j["actions"].as_array().unwrap()) {
        assert_eq!(raw["id"], a.id.as_str());
        assert!(tinhat_actions::action_by_id(&a.id).is_some_and(|b| std::ptr::eq(a, b)), "{}", a.id);
    }
    for (reg, n) in [
        ("predicates", t.predicates.len()),
        ("states", t.states.len()),
        ("valueTypes", t.value_types.len()),
        ("targetKinds", t.target_kinds.len()),
        ("scopes", t.scopes.len()),
        ("rows", t.rows.len()),
        ("menus", t.menus.len()),
    ] {
        assert_eq!(j[reg].as_object().unwrap().len(), n, "{reg}");
    }
    assert_eq!(tinhat_actions::table::scope_chain("composer"), vec!["composer", "main", "app"]);
    assert_eq!(tinhat_actions::table::scope_chain("nowhere"), vec!["nowhere"]);
    assert!(tinhat_actions::table::scopes_overlap("composer", "app") && !tinhat_actions::table::scopes_overlap("composer", "conversation"));
}

#[test]
fn call_answers_canonical_json_and_errors_for_bad_text_and_unknown_kinds() {
    assert_eq!(call("fold", r#"{"text": "Gaming  Central"}"#), r#"{"folded":"gaming central"}"#);
    for bad in ["", " ", "{", "nope", "[1,]", "{\"text\": \"\\ud800\"}"] {
        for k in KINDS {
            assert_eq!(call(k, bad), r#"{"error":"bad-input"}"#, "{k} {bad:?}");
        }
    }
    let deep = "[".repeat(200) + &"]".repeat(200);
    assert_eq!(call("evaluate", &deep), r#"{"error":"bad-input"}"#);
    assert_eq!(call("teleport", "{}"), r#"{"error":"unknown-kind"}"#);
    assert!(dispatch("teleport", &json!({})).is_none());
    // -0 is 0 (checkParam), integral doubles print like JSON.stringify, keys in code-point order
    let p = call("target", r#"{"action": "person.volume.set", "target": {"level": -0, "category": "mic"}}"#);
    assert_eq!(p, r#"{"ok":true,"target":{"category":"mic","level":0}}"#);
    let p = call("target", r#"{"action": "person.volume.set", "target": {"level": 1.5e2}}"#);
    assert_eq!(p, r#"{"ok":true,"target":{"level":150}}"#);
    let p = call("target", r#"{"action": "person.volume.set", "target": {"level": 0.5}}"#);
    assert_eq!(p, r#"{"error":"bad-value","key":"level","ok":false}"#);
}

// ---- robustness: no entry point panics, whatever it is given ---------------------------------------------------------

/// The reference's generator: xorshift32 (13/17/5) from 0x9e3779b9, `(x >>> 0) / 2^32`.
struct Rnd(u32);

impl Rnd {
    fn next(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x as f64 / 4294967296.0
    }
    fn pick<T: Clone>(&mut self, xs: &[T]) -> T {
        let i = (self.next() * xs.len() as f64).floor() as usize;
        xs[i.min(xs.len() - 1)].clone()
    }
    fn int(&mut self, n: usize) -> usize {
        ((self.next() * n as f64).floor() as usize).min(n.saturating_sub(1))
    }
}

fn junk() -> Vec<Value> {
    vec![
        Value::Null,
        json!(true),
        json!(3),
        json!(-0.0),
        json!(1e308),
        json!(""),
        json!("x"),
        json!("!room:example.com"),
        json!([]),
        json!([1, "a"]),
        json!({}),
        json!({"constructor": 1}),
    ]
}

fn all_entry_points(input: &Value) {
    for k in KINDS {
        let out = dispatch(k, input).expect("an entry point");
        assert!(out.is_object(), "{k}: {}", canon_json(&out));
    }
}

#[test]
fn robustness_no_entry_point_panics_whatever_it_is_given() {
    let mut r = Rnd(0x9e3779b9);
    let junk = junk();
    let mut ids: Vec<Value> = table().actions.iter().take(50).map(|a| json!(a.id)).collect();
    ids.push(json!("x.y"));
    ids.push(json!(7));
    for _ in 0..3000 {
        let app = r.pick(&[json!("desktop"), json!("web"), json!("x"), json!(1)]);
        let action = r.pick(&ids);
        let inner = json!({"values": r.pick(&junk), "subjects": r.pick(&junk), "current": r.pick(&junk), "local": r.pick(&junk)});
        let mut snaps = junk.clone();
        snaps.push(inner);
        let snapshot = r.pick(&snaps);
        let room_t = json!({"room": r.pick(&junk)});
        let local_t = json!({"local": r.pick(&junk)});
        let mut targets = junk.clone();
        targets.push(room_t);
        targets.push(local_t);
        let target = r.pick(&targets);
        let context = r.pick(&junk);
        let params = r.pick(&junk);
        let row = r.pick(&[None, Some("buttons.main"), Some("x")]);
        let mut input = json!({"app": app, "action": action, "snapshot": snapshot, "target": target, "context": context, "params": params});
        if let Some(rw) = row {
            input["row"] = json!(rw);
        }
        for k in KINDS {
            assert!(dispatch(k, &input).is_some_and(|o| o.is_object()));
            let j = r.pick(&junk);
            assert!(dispatch(k, &j).is_some_and(|o| o.is_object()));
        }
    }
}

/// A random JSON value over the engine's vocabulary: its keys, action / row / menu / subject ids, odd strings and
/// numbers, nested up to `depth`.
fn random_value(r: &mut Rnd, depth: usize, vocab: &[Value], keys: &[&str]) -> Value {
    let roll = r.int(if depth == 0 { 6 } else { 9 });
    match roll {
        0 => Value::Null,
        1 => json!(r.next() < 0.5),
        2 => r.pick(&[json!(0), json!(-0.0), json!(1), json!(-1), json!(2.5), json!(24), json!(1e21), json!(-1e308), json!(4294967296u64)]),
        3..=5 => r.pick(vocab),
        6 | 7 => {
            let mut o = Map::new();
            for _ in 0..r.int(6) {
                let k = r.pick(keys).to_string();
                o.insert(k, random_value(r, depth - 1, vocab, keys));
            }
            Value::Object(o)
        }
        _ => Value::Array((0..r.int(5)).map(|_| random_value(r, depth - 1, vocab, keys)).collect()),
    }
}

fn vocabulary() -> (Vec<Value>, Vec<&'static str>) {
    let t = table();
    let mut vocab: Vec<Value> = Vec::new();
    for a in t.actions.iter().step_by(3) {
        vocab.push(json!(a.id));
    }
    for k in t.rows.keys().chain(t.menus.keys()).chain(t.target_kinds.keys()).chain(t.predicates.keys().take(40)).chain(t.states.keys()) {
        vocab.push(json!(k));
    }
    for s in [
        "desktop", "web", "phone", "", " ", "x", "!room:example.com", "!space:example.com", "@zane:example.com", "@a", "@a:", "#ff00AA",
        "#FF00aa", "#ff00a", "#gg0000", "é", "🎯", "\u{0}", "\u{7f}", "\u{2028}", "a\tb", "{name}", "{prefs.theme}", "{room.pinnedCount}",
        "mxc://x/y", "https://x", "home", "lfg", "goose", "h1", "abcdefghijklm", "Σ", "İ", ">", "@", "#", " >mute", "mute", "mic",
        "iconLabel", "label", "icon", "S", "M", "name", "keyword", "alias", "all", "any", "not", "eq", "param",
    ] {
        vocab.push(json!(s));
    }
    let keys: Vec<&'static str> = vec![
        "app", "action", "snapshot", "target", "context", "params", "row", "menu", "item", "form", "transient", "value", "query",
        "stage", "text", "as", "values", "subjects", "current", "local", "lists", "room", "space", "person", "rail", "status",
        "theme", "pane", "sound", "device", "source", "value", "category", "level", "emoji", "items", "a", "hideWhenOff",
        "sepBefore", "look", "scale", "name", "label", "alias", "activeSpace", "room.joined", "call.in", "call.micMuted",
        "__proto__", "constructor", "toString", "",
    ];
    (vocab, keys)
}

/// Structural fuzz: random inputs over the engine's vocabulary through every entry point, typed and as text (the
/// canonical text, and a cut / corrupted copy of it) — every answer is an object, nothing panics.
#[test]
fn robustness_structural_fuzz() {
    let (vocab, keys) = vocabulary();
    let mut r = Rnd(0x2545f491);
    for i in 0..4000 {
        let mut input = random_value(&mut r, 5, &vocab, &keys);
        // most inputs are objects with a valid app and a real entry-point shape, so the fuzz reaches the engine
        if i % 4 != 0 {
            if !input.is_object() {
                input = json!({});
            }
            input["app"] = r.pick(&[json!("desktop"), json!("web")]);
            if r.next() < 0.7 {
                input["snapshot"] = random_value(&mut r, 5, &vocab, &keys);
            }
        }
        all_entry_points(&input);
        let text = canon_json(&input);
        let k = KINDS[r.int(KINDS.len())];
        assert!(call(k, &text).starts_with('{'));
        let cut = text.char_indices().nth(r.int(text.chars().count().max(1))).map(|(b, _)| b).unwrap_or(0);
        assert!(call(k, &text[..cut]).starts_with('{'));
    }
}

fn vector_inputs() -> Vec<(String, Value)> {
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        let mut v: Vec<PathBuf> = std::fs::read_dir(d).unwrap().map(|e| e.unwrap().path()).collect();
        v.sort();
        for p in v {
            if p.is_dir() {
                walk(&p, out)
            } else {
                out.push(p)
            }
        }
    }
    let mut files = Vec::new();
    walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("testvectors/actions"), &mut files);
    let mut out = Vec::new();
    for f in files.iter().filter(|p| p.extension().is_some_and(|e| e == "json")) {
        let doc = parse_json(&std::fs::read_to_string(f).unwrap()).unwrap();
        let kind = doc["kind"].as_str().unwrap().to_string();
        for c in doc["cases"].as_array().unwrap().iter().step_by(7) {
            out.push((kind.clone(), c["input"].clone()));
        }
    }
    out
}

/// Replace one random node of `v` (walking down at most `depth` levels) with `with`.
fn mutate(r: &mut Rnd, v: &mut Value, with: Value, depth: usize) {
    if depth == 0 || r.next() < 0.3 {
        *v = with;
        return;
    }
    match v {
        Value::Object(o) if !o.is_empty() => {
            let keys: Vec<String> = o.keys().cloned().collect();
            let k = r.pick(&keys);
            if r.next() < 0.15 {
                o.remove(&k);
            } else if let Some(x) = o.get_mut(&k) {
                mutate(r, x, with, depth - 1);
            }
        }
        Value::Array(a) if !a.is_empty() => {
            let i = r.int(a.len());
            mutate(r, &mut a[i], with, depth - 1);
        }
        _ => *v = with,
    }
}

/// Every seventh vector input, mutated three ways (a node replaced by junk / by a vocabulary value / removed), through
/// its own entry point and every other one — nothing panics, every answer is an object.
#[test]
fn robustness_mutated_vector_inputs() {
    let (vocab, keys) = vocabulary();
    let mut r = Rnd(0x6a09e667);
    let inputs = vector_inputs();
    assert!(inputs.len() > 2000, "{} inputs", inputs.len());
    for (kind, input) in &inputs {
        for _ in 0..3 {
            let mut m = input.clone();
            let with = random_value(&mut r, 3, &vocab, &keys);
            mutate(&mut r, &mut m, with, 6);
            assert!(dispatch(kind, &m).is_some_and(|o| o.is_object()));
        }
        let other = KINDS[r.int(KINDS.len())];
        assert!(dispatch(other, input).is_some_and(|o| o.is_object()));
    }
}
