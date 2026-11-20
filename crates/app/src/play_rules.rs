//! The mini games' pure client rules (GAMES-V1-SPEC rev 2: §1.1, §3.1, §3.3, §6.5), replayed against
//! `testvectors/client-rules.json`. Tin Hat Web's `src/features/prestige/clientRules.ts` replays a byte copy of the
//! same file.
//!
//!   prestige_tier(rank)      rank → the token's look (rank 0 / anything that isn't a JSON integer ≥ 1: no token)
//!   game_visible(game, ctx)  is a game offered here (the room server's features.play + the phone-app switch)
//!   game_space(ctx)          which space's chips a room uses (both links, canonical first, unknown blocks; the owner =
//!                            the server of the space's create-event sender, never a via)
//!   rank_context(surface)    which space's rank a token shows: a room = its game space, a space's own surfaces = that
//!                            space, anything else = none
//!   whisper_alone(ctx)       The Rig: warn before a Whisper when the whisperer is the only connected House member
//!
//! No Qt, no I/O: the games UI (game_visible, game_space) and the token (prestige_tier, rank_context) use this file.

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrestigeLook {
    /// 0 = no token; 1 outline; 2 filled; 3 accent; 4 + ring; 5 + star; 6 + the one-time shine
    pub tier: u8,
    /// "none" | "outline" | "filled"
    pub glyph: &'static str,
    /// "none" | "muted" | "text" | "accent"
    pub colour: &'static str,
    pub ring: bool,
    pub star: bool,
    pub shine: bool,
}

pub const NO_PRESTIGE: PrestigeLook = PrestigeLook { tier: 0, glyph: "none", colour: "none", ring: false, star: false, shine: false };

/// The tier ladder (§3.1). Each row starts at its rank; the last row whose start ≤ rank wins.
const LADDER: [(i64, PrestigeLook); 6] = [
    (1, PrestigeLook { tier: 1, glyph: "outline", colour: "muted", ring: false, star: false, shine: false }),
    (2, PrestigeLook { tier: 2, glyph: "filled", colour: "text", ring: false, star: false, shine: false }),
    (3, PrestigeLook { tier: 3, glyph: "filled", colour: "accent", ring: false, star: false, shine: false }),
    (5, PrestigeLook { tier: 4, glyph: "filled", colour: "accent", ring: true, star: false, shine: false }),
    (8, PrestigeLook { tier: 5, glyph: "filled", colour: "accent", ring: true, star: true, shine: false }),
    (13, PrestigeLook { tier: 6, glyph: "filled", colour: "accent", ring: true, star: true, shine: true }),
];

/// JavaScript's Number.MAX_SAFE_INTEGER: the web side's `Number.isSafeInteger` bound, kept equal here.
const MAX_SAFE: i64 = 9_007_199_254_740_991;

/// A rank as it travels (JSON): only an integer ≥ 1 is a rank; 2.5, "3", true, null, negatives are none (0).
pub fn valid_rank(v: &Value) -> i64 {
    // serde_json keeps an integer literal as i64/u64 and anything with a fraction or exponent as f64.
    // An f64 that happens to be whole (2.0) is accepted, as JavaScript sees no difference.
    let n = match v {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i
            } else if let Some(f) = n.as_f64() {
                if f.fract() == 0.0 && f.abs() <= MAX_SAFE as f64 {
                    f as i64
                } else {
                    return 0;
                }
            } else {
                return 0;
            }
        }
        _ => return 0,
    };
    if (1..=MAX_SAFE).contains(&n) {
        n
    } else {
        0
    }
}

pub fn prestige_tier(rank: i64) -> PrestigeLook {
    let mut look = NO_PRESTIGE;
    if rank < 1 || rank > MAX_SAFE {
        return look;
    }
    for (from, l) in LADDER {
        if rank >= from {
            look = l;
        }
    }
    look
}

pub fn prestige_tier_json(rank: &Value) -> PrestigeLook {
    prestige_tier(valid_rank(rank))
}

/// What the token says to a screen reader and in its tooltip ("Prestige 4"); "" for no token.
pub fn prestige_label(rank: i64) -> String {
    if prestige_tier(rank).tier > 0 {
        format!("Prestige {rank}")
    } else {
        String::new()
    }
}

/// `ctx` = {play, phone_app, rig_phone_apps} as JSON (each must be exactly `true` to count).
pub fn game_visible(game: &str, ctx: &Value) -> bool {
    let is_true = |k: &str| ctx.get(k) == Some(&Value::Bool(true));
    if !is_true("play") {
        return false;
    }
    match game {
        "holdem" => true,
        "rig" => !is_true("phone_app") || is_true("rig_phone_apps"),
        _ => false,
    }
}

/// The server part of a Matrix user id: everything after the first ':' (a port is kept); None when malformed.
pub fn server_of_user_id(mxid: &str) -> Option<&str> {
    if mxid.len() > 255 {
        return None;
    }
    let rest = mxid.strip_prefix('@')?;
    let i = rest.find(':')?;
    if i == 0 {
        return None;
    }
    let server = &rest[i + 1..];
    if server.is_empty() || server.chars().any(char::is_whitespace) {
        return None;
    }
    Some(server)
}

// ---- game_space (§1.1) ---------------------------------------------------------------------------------------------

/// A link's `via`: a list of 1–20 strings of 1–255 characters (an empty list = a removed link). Lengths count
/// characters (code points), as the reference does.
pub fn via_ok(content: &Value) -> bool {
    let Some(v) = content.as_object().and_then(|o| o.get("via")).and_then(Value::as_array) else { return false };
    (1..=20).contains(&v.len()) && v.iter().all(|x| x.as_str().is_some_and(|s| (1..=255).contains(&s.chars().count())))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameSpace {
    pub space: Option<String>,
    pub server: Option<String>,
    /// "" | "is-space" | "no-space" | "unavailable" | "wrong-server"
    pub why: &'static str,
}

fn none(why: &'static str) -> GameSpace {
    GameSpace { space: None, server: None, why }
}

/// The vector's shape: `ctx` → `{"space", "server", "why"}` (what games-desktop and the server's vectors compare).
pub fn game_space(ctx: &Value) -> Value {
    let g = game_space_of(ctx);
    serde_json::json!({"space": g.space, "server": g.server, "why": g.why})
}

/// `ctx` = {mode: "app"|"server", decider?, room: {id, create_type?, parents: [{space, content}]}, spaces: {id:
/// {create_sender, create_type, partial?, children: [{room, content}]}}} — only the spaces whose CURRENT, full state
/// the caller holds. Space ids compare by code point (Rust's str order is code-point order).
pub fn game_space_of(ctx: &Value) -> GameSpace {
    let room = &ctx["room"];
    if room.get("create_type").and_then(Value::as_str) == Some("m.space") {
        return none("is-space");
    }
    let room_id = room.get("id").and_then(Value::as_str).unwrap_or("");
    let spaces = ctx.get("spaces").and_then(Value::as_object);
    let mut cands: Vec<(&str, bool)> = room
        .get("parents")
        .and_then(Value::as_array)
        .map(|ps| {
            ps.iter()
                .filter_map(|p| {
                    let space = p.get("space")?.as_str()?;
                    let content = p.get("content")?;
                    (space.starts_with('!') && via_ok(content)).then(|| (space, content.get("canonical") == Some(&Value::Bool(true))))
                })
                .collect()
        })
        .unwrap_or_default();
    cands.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    for (space, _) in cands {
        let Some(sp) = spaces.and_then(|m| m.get(space)) else { return none("unavailable") };
        if sp.get("partial").is_some_and(|v| v.as_bool().unwrap_or(!v.is_null())) {
            return none("unavailable"); // unknown blocks: never fall through it
        }
        if sp.get("create_type").and_then(Value::as_str) != Some("m.space") {
            continue; // an ordinary room's m.space.child means nothing
        }
        let lists_back = sp.get("children").and_then(Value::as_array).is_some_and(|cs| {
            cs.iter().any(|c| c.get("room").and_then(Value::as_str) == Some(room_id) && c.get("content").is_some_and(via_ok))
        });
        if lists_back {
            let Some(server) = sp.get("create_sender").and_then(Value::as_str).and_then(server_of_user_id) else {
                return none("unavailable");
            };
            if ctx.get("mode").and_then(Value::as_str) == Some("server") && Some(server) != ctx.get("decider").and_then(Value::as_str) {
                return GameSpace { space: Some(space.to_string()), server: Some(server.to_string()), why: "wrong-server" };
            }
            return GameSpace { space: Some(space.to_string()), server: Some(server.to_string()), why: "" };
        }
        // a known space that doesn't list the room back: skip
    }
    none("no-space")
}

// ---- rank_context (§3.3) -------------------------------------------------------------------------------------------

/// Which space's rank a token on this surface shows (None = no token): {kind:"room", game_space} → that space,
/// {kind:"space", space} → that space, anything else (account card, Settings, search, DMs) → none.
pub fn rank_context(surface: &Value) -> Option<String> {
    match surface.get("kind").and_then(Value::as_str) {
        Some("space") => surface.get("space").and_then(Value::as_str).map(str::to_string),
        Some("room") => surface.get("game_space").and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
}

// ---- whisper_alone (launch-gate 11-19: never promise anonymity when it isn't true) --------------------------------

/// The House roles (the Spinner is not House).
pub const HOUSE_ROLES: [&str; 3] = ["mechanic", "sharp", "railbird"];

/// `ctx` = {roster: [{u, role, connected, left?}], whisperer}: warn when no OTHER House member is connected (exactly
/// `true`) and still in the round (`left` not `true`) — the Spinner would know who wrote it. The words (both apps):
/// "You're the only House member here, so the Spinner will know this is from you."
pub fn whisper_alone_warn(ctx: &Value) -> bool {
    let whisperer = ctx.get("whisperer").and_then(Value::as_str).unwrap_or("");
    let others = ctx
        .get("roster")
        .and_then(Value::as_array)
        .map(|r| {
            r.iter()
                .filter(|m| m.get("u").and_then(Value::as_str) != Some(whisperer))
                .filter(|m| m.get("role").and_then(Value::as_str).is_some_and(|role| HOUSE_ROLES.contains(&role)))
                .filter(|m| m.get("connected") == Some(&Value::Bool(true)))
                .filter(|m| m.get("left") != Some(&Value::Bool(true)))
                .count()
        })
        .unwrap_or(0);
    others == 0
}

/// The vector's shape: `ctx` → `{"warn": bool}` (= the web's whisperAlone).
pub fn whisper_alone(ctx: &Value) -> Value {
    serde_json::json!({"warn": whisper_alone_warn(ctx)})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vectors() -> Value {
        serde_json::from_str(include_str!("testvectors/client-rules.json")).unwrap()
    }

    #[test]
    fn vectors_are_version_1_with_every_rule() {
        let v = vectors();
        assert_eq!(v["v"], 1);
        assert!(v["prestige_tier"]["cases"].as_array().unwrap().len() >= 16);
        assert!(v["game_visible"]["cases"].as_array().unwrap().len() >= 10);
        assert!(v["game_space"]["cases"].as_array().unwrap().len() >= 27);
        assert!(v["whisper_alone"]["cases"].as_array().unwrap().len() >= 8);
        assert!(v["rank_context"]["cases"].as_array().unwrap().len() >= 4);
        assert!(v.get("rank_server").is_none(), "rev 2 removed rank_server");
    }

    #[test]
    fn prestige_tier_vectors() {
        for c in vectors()["prestige_tier"]["cases"].as_array().unwrap() {
            let got = prestige_tier_json(&c["rank"]);
            let e = &c["expect"];
            let want = (
                e["tier"].as_u64().unwrap() as u8,
                e["glyph"].as_str().unwrap(),
                e["colour"].as_str().unwrap(),
                e["ring"].as_bool().unwrap(),
                e["star"].as_bool().unwrap(),
                e["shine"].as_bool().unwrap(),
            );
            assert_eq!((got.tier, got.glyph, got.colour, got.ring, got.star, got.shine), want, "rank {}", c["rank"]);
        }
    }

    #[test]
    fn prestige_ladder_is_monotone() {
        let mut prev = 0u8;
        for r in 0..=60 {
            let t = prestige_tier(r).tier;
            assert!(t >= prev && t - prev <= 1, "rank {r}: {prev} -> {t}");
            prev = t;
        }
        assert_eq!(prev, 6);
    }

    #[test]
    fn prestige_label_words() {
        assert_eq!(prestige_label(4), "Prestige 4");
        assert_eq!(prestige_label(13), "Prestige 13");
        assert_eq!(prestige_label(0), "");
        assert_eq!(prestige_label(-1), "");
    }

    #[test]
    fn game_visible_vectors() {
        for c in vectors()["game_visible"]["cases"].as_array().unwrap() {
            let got = game_visible(c["game"].as_str().unwrap(), &c["ctx"]);
            assert_eq!(got, c["expect"].as_bool().unwrap(), "{}", c["id"]);
        }
    }

    #[test]
    fn game_space_vectors() {
        for c in vectors()["game_space"]["cases"].as_array().unwrap() {
            assert_eq!(game_space(&c["ctx"]), c["expect"], "{}", c["id"]);
        }
    }

    #[test]
    fn whisper_alone_vectors() {
        for c in vectors()["whisper_alone"]["cases"].as_array().unwrap() {
            let ctx = serde_json::json!({"roster": c["roster"], "whisperer": c["whisperer"]});
            assert_eq!(whisper_alone(&ctx), c["expect"], "{}", c["id"]);
        }
    }

    #[test]
    fn rank_context_vectors() {
        for c in vectors()["rank_context"]["cases"].as_array().unwrap() {
            assert_eq!(rank_context(&c["surface"]).as_deref(), c["expect"]["space"].as_str(), "{}", c["id"]);
        }
    }

    /// A space that lists only OTHER rooms is not this room's space (a case the shared vectors lack; = the web's test).
    #[test]
    fn game_space_space_listing_only_other_rooms() {
        let ctx = serde_json::json!({
            "mode": "app",
            "room": {"id": "!r", "parents": [{"space": "!S", "content": {"via": ["x.test"]}}]},
            "spaces": {"!S": {"create_sender": "@a:x.test", "create_type": "m.space", "children": [{"room": "!other", "content": {"via": ["x.test"]}}]}}
        });
        assert_eq!(game_space_of(&ctx), GameSpace { space: None, server: None, why: "no-space" });
    }

    #[test]
    fn game_space_helpers() {
        // 255 is counted in characters (code points): 128 emoji pass, 256 don't (= the web's [...x].length)
        assert!(via_ok(&serde_json::json!({"via": ["\u{1f600}".repeat(128)]})));
        assert!(!via_ok(&serde_json::json!({"via": ["\u{1f600}".repeat(256)]})));
        assert!(via_ok(&serde_json::json!({"via": ["a"]})));
        assert!(via_ok(&serde_json::json!({"via": vec!["a"; 20]})));
        for bad in [
            serde_json::json!({"via": []}),
            serde_json::json!({"via": vec!["a"; 21]}),
            serde_json::json!({"via": ["x".repeat(256)]}),
            serde_json::json!({"via": "a"}),
            serde_json::json!({"via": [7]}),
            serde_json::json!({}),
            Value::Null,
        ] {
            assert!(!via_ok(&bad), "{bad}");
        }
        // code-point order: U+FF5E before U+1F600 (the web compares code points the same way)
        assert!("!\u{ff5e}" < "!\u{1f600}");
    }

    #[test]
    fn server_of_user_id_cases() {
        assert_eq!(server_of_user_id("@a:b.c"), Some("b.c"));
        assert_eq!(server_of_user_id("@a:b.c:8448"), Some("b.c:8448"));
        for bad in ["a:b", "@:b", "@a:", "@a", "", "@a:b c"] {
            assert_eq!(server_of_user_id(bad), None, "{bad}");
        }
    }

    /// The byte copy equals Tin Hat Web's when a web checkout is named by `TINHAT_WEB_DIR` (skipped otherwise).
    #[test]
    fn vectors_are_the_webs() {
        let Some(dir) = std::env::var_os("TINHAT_WEB_DIR") else { return };
        let path = std::path::Path::new(&dir).join("src/features/prestige/testvectors/client-rules.json");
        let theirs = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(include_str!("testvectors/client-rules.json"), theirs);
    }
}
