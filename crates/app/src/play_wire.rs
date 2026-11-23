//! play_wire.rs — Tin Hat mini games v1 (chips, Texas Hold'em, The Rig): the PURE side of the play client.
//! No Qt, no network, no clock: every function is a deterministic function of its arguments, so the web's
//! `features/games/` is held to the same shared vectors (`testvectors/client-rules.json`, `testvectors/games/*`, byte-identical in
//! both apps and the spec's `vectors/`).
//!
//! What lives here (the spec is GAMES-V1-SPEC.md; § numbers refer to it):
//!   (gameVisible / game_space / rank_context: play_rules.rs, the one copy)
//!   • `check_view`      §4.3 / §5.3 — a `view` snapshot is checked before QML ever sees it: a malformed view is
//!                       dropped (and logged), never half-drawn. Extra keys are allowed (the contract only grows).
//!   • `Newest`          §6.3 — "keep the newest vseq": an older or repeated snapshot never replaces a newer one
//!   • `new_op_id`       §2.5 — the move's idempotency id (16–32 of `[A-Za-z0-9_-]`)
//!   • `card_face`       the label of a playing card ("Td" → 10 ♦, red)
//!   • `frame_ok`        §6.3 wire hygiene for frames we receive (≤ 16 KiB, depth ≤ 32, integers ≤ 2^53−1)
//!   • `post_body`       §6.6 — the plain-text body of a game card (no amounts, ever)
//!
//! The server is the only judge of what is legal (`you.legal`); nothing here computes a game rule.

use serde_json::{json, Map, Value};

/// The games v1 knows. Anything else is hidden (client-rules `unknown-game`).
pub const GAMES: [&str; 2] = ["holdem", "rig"];

/// The custom block a game card carries in an `m.room.message` (§6.6).
pub const CARD_KEY: &str = "dev.tinhat.game";

/// Largest integer JavaScript can hold exactly (2^53 − 1): the wire's integer limit (§6.3).
pub const MAX_SAFE_INT: u64 = 9_007_199_254_740_991;

/// Frames above this are refused (§6.3: ≤ 16 KiB).
pub const MAX_FRAME: usize = 16 * 1024;

// ---------------------------------------------------------------------------------------------------------------
// The shared client rules (gameVisible, game_space, rank_context) are `play_rules.rs` (one copy per app).
// ---------------------------------------------------------------------------------------------------------------

// ---------------------------------------------------------------------------------------------------------------
// §2.5 op ids
// ---------------------------------------------------------------------------------------------------------------

/// A fresh move id: 16 random bytes as unpadded base64url = 22 characters of `[A-Za-z0-9_-]` (the contract's
/// 16–32). The OS CSPRNG, like the Pad's ids.
pub fn new_op_id() -> String {
    use base64::Engine as _;
    use rand::RngCore as _;
    let mut b = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut b);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

/// The contract's op id shape (the server checks it too; we never send another).
pub fn op_id_ok(s: &str) -> bool {
    (16..=32).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

// ---------------------------------------------------------------------------------------------------------------
// §6.3 frame hygiene + the newest vseq
// ---------------------------------------------------------------------------------------------------------------

fn depth_ok(v: &Value, depth: usize) -> bool {
    if depth > 32 {
        return false;
    }
    match v {
        Value::Array(a) => a.iter().all(|x| depth_ok(x, depth + 1)),
        Value::Object(o) => o.values().all(|x| depth_ok(x, depth + 1)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.unsigned_abs() <= MAX_SAFE_INT
            } else if let Some(u) = n.as_u64() {
                u <= MAX_SAFE_INT
            } else {
                // a float written for an integer (9007199254740992.0, 1e30) is held to the same bound
                n.as_f64().is_some_and(|f| f.is_finite() && (f.fract() != 0.0 || f.abs() <= MAX_SAFE_INT as f64))
            }
        }
        _ => true,
    }
}

/// Parse one received text frame: ≤ 16 KiB, JSON object with a string `t`, nesting ≤ 32, integers within
/// JavaScript's safe range. serde_json already refuses lone surrogates in `\u` escapes.
pub fn frame_ok(text: &str) -> Option<Value> {
    if text.len() > MAX_FRAME {
        return None;
    }
    let v: Value = serde_json::from_str(text).ok()?;
    if !v.get("t").is_some_and(Value::is_string) || !depth_ok(&v, 0) {
        return None;
    }
    Some(v)
}

/// Every code the contract knows (schema.json `x-codes`, §6.3 + rev 2).
pub const CODES: [&str; 44] = [
    "session", "guest", "not-member", "not-in-call", "remote-refused", "rig-hidden", "no-space", "wrong-server",
    "space-changed", "unavailable", "no-table", "no-wallet", "seated-elsewhere", "table-full", "one-per-room", "seated",
    "open-money", "rate", "off", "db", "stale", "not-your-turn", "phase", "bad", "bad-action", "bad-amount", "no-chips",
    "limit", "locked", "window", "taken", "rules", "greyed", "host-only",
    // round 2 (contract v1.1b, GAMES-V1.1-ADDENDUM §4.2 / §5 / §6)
    "cooldown", "op-reused", "bad-reason", "bad-value", "at-table", "not-admin", "not-broke", "day-max", "side-off", "no-run",
];

/// A known server frame type with a wrong field is refused (dropped whole); unknown types and extra fields are
/// tolerated (the contract's malformed.json `tolerated`). Views go through [`check_view`] as well.
pub fn server_frame_ok(f: &Value) -> bool {
    match f.get("t").and_then(Value::as_str) {
        Some("ack") => {
            f.get("op").and_then(Value::as_str).is_some_and(op_id_ok)
                && f.get("ok").is_some_and(Value::is_boolean)
                && f.get("code").and_then(Value::as_str).is_none_or(|c| c.is_empty() || CODES.contains(&c))
        }
        Some("rank") => f.get("space").and_then(Value::as_str).is_some_and(|s| s.starts_with('!')) && mxid_ok(f.get("u")) && uint(f.get("rank")).is_some(),
        Some("wallet") => f.get("space").and_then(Value::as_str).is_some_and(|s| s.starts_with('!')),
        Some("view") => f.get("view").is_some_and(|v| check_view(v).is_ok()),
        // round 2 F7: {t:"react", table, u, seat: n|null, r, to: n|"pot"|absent, at}
        Some("react") => {
            f.get("table").and_then(Value::as_str).is_some_and(table_id_ok)
                && mxid_ok(f.get("u"))
                && f.get("r").and_then(Value::as_str).is_some_and(|r| crate::games_rules::REACTIONS.contains(&r))
                && f.get("seat").is_none_or(|s| s.is_null() || uint(Some(s)).is_some())
                && f.get("to").is_none_or(|t| t.is_null() || t == "pot" || uint(Some(t)).is_some())
        }
        Some("error") | Some("bye") => f.get("code").is_none_or(Value::is_string),
        Some(_) => true,
        None => false,
    }
}

/// "Keep the newest vseq" per table: an older or repeated snapshot is ignored.
#[derive(Default, Debug)]
pub struct Newest {
    seen: std::collections::HashMap<String, u64>,
}

impl Newest {
    /// True when `vseq` is newer than anything kept for `table` (and records it).
    pub fn accept(&mut self, table: &str, vseq: u64) -> bool {
        match self.seen.get(table) {
            Some(&last) if vseq <= last => false,
            _ => {
                self.seen.insert(table.to_owned(), vseq);
                true
            }
        }
    }
    /// Forget a table (closed, or the socket reconnected: the server's vseq may restart after its own restart).
    pub fn forget(&mut self, table: &str) {
        self.seen.remove(table);
    }
    pub fn clear(&mut self) {
        self.seen.clear();
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Cards and symbols
// ---------------------------------------------------------------------------------------------------------------

/// A playing card as the wire writes it: rank `23456789TJQKA` + suit `cdhs` ("Td").
pub fn card_ok(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 2 && b"23456789TJQKA".contains(&b[0]) && b"cdhs".contains(&b[1])
}

/// The face of a card for drawing: `{"rank":"10","suit":"♦","red":true}` (None for a malformed card).
pub fn card_face(s: &str) -> Option<Value> {
    if !card_ok(s) {
        return None;
    }
    let b = s.as_bytes();
    let rank = if b[0] == b'T' { "10".to_owned() } else { (b[0] as char).to_string() };
    let (suit, red) = match b[1] {
        b'c' => ("♣", false),
        b'd' => ("♦", true),
        b'h' => ("♥", true),
        _ => ("♠", false),
    };
    Some(json!({"rank": rank, "suit": suit, "red": red}))
}

/// The Rig's six symbols in ring order (§5.0; a Nudge steps round this ring).
pub const RIG_SYMBOLS: [&str; 6] = ["hat", "saucer", "dish", "eye", "moon", "lizard"];

/// The Rig's sixteen cards in deck order (§5.0 card table; ids as the wire and the vectors write them).
pub const RIG_CARDS: [&str; 16] = [
    "moon-monday",
    "lizard-lane",
    "hat-thief",
    "twos-company",
    "big-bet-tax",
    "penny-pincher",
    "cold-streak",
    "skim",
    "moon-tax",
    "shy-hat",
    "lizard-tax",
    "underdog",
    "rebound",
    "hot-hat",
    "slow-start",
    "even-money",
];

// ---------------------------------------------------------------------------------------------------------------
// §4.3 / §5.3 view checks
// ---------------------------------------------------------------------------------------------------------------

fn uint(v: Option<&Value>) -> Option<u64> {
    v.and_then(Value::as_u64).filter(|&n| n <= MAX_SAFE_INT)
}

fn mxid_ok(v: Option<&Value>) -> bool {
    v.and_then(Value::as_str).is_some_and(|s| crate::play_rules::server_of_user_id(s).is_some() && s.len() <= 255)
}

/// A table id: 1–64 of `[A-Za-z0-9_-]`.
/// Round 2 (GAMES-V1.1-ADDENDUM §5.3, §6.4): the ONLY routes QML may call through PlayController.playApi:
/// Tune In (`POST /side/tune`, `POST /side/tune/{run}`), the admin screen (`GET /admin/spaces/{space}`,
/// `POST …/wallets`, `PUT …/economy`, `GET …/log[?before=n&limit=n]`), `POST /me/notices/seen`. `{space}` = one
/// percent-encoded room id segment; nothing else (no `..`, no other query, no other method).
pub fn r2_route_ok(method: &str, path: &str) -> bool {
    // one room-id segment as encodeURIComponent writes it ('!' stays, ':' → %3A); never a dot segment in any spelling
    // (%2e%2e, .%2e, %2e. normalise to ".." in URL parsing), never an encoded / or \
    fn seg_ok(s: &str, max: usize) -> bool {
        let l = s.to_ascii_lowercase();
        !s.is_empty() && s.len() <= max
            && s.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'%' | b'.' | b'_' | b'-' | b'!'))
            && !l.contains("%2e") && !l.contains("%2f") && !l.contains("%5c") && !l.contains("%00")
            && s != "." && !s.contains("..")
    }
    let (p, q) = match path.split_once('?') { Some((p, q)) => (p, Some(q)), None => (path, None) };
    let parts: Vec<&str> = p.split('/').collect();
    if parts.first() != Some(&"") {
        return false;
    }
    let q_ok = |allowed: &[&str]| -> bool {
        match q {
            None => true,
            Some(q) => q.split('&').all(|kv| match kv.split_once('=') {
                Some((k, v)) => allowed.contains(&k) && !v.is_empty() && v.len() <= 20 && v.bytes().all(|c| c.is_ascii_digit()),
                None => false,
            }),
        }
    };
    match (method, &parts[1..]) {
        ("POST", ["side", "tune"]) => q.is_none(),
        ("POST", ["side", "tune", run]) => q.is_none() && run.len() <= 40 && run.starts_with("sR") && run[2..].bytes().all(|c| c.is_ascii_alphanumeric()),
        ("GET", ["admin", "spaces", sp]) => q.is_none() && seg_ok(sp, 300),
        ("POST", ["admin", "spaces", sp, "wallets"]) => q.is_none() && seg_ok(sp, 300),
        ("PUT", ["admin", "spaces", sp, "economy"]) => q.is_none() && seg_ok(sp, 300),
        ("GET", ["admin", "spaces", sp, "log"]) => seg_ok(sp, 300) && q_ok(&["before", "limit"]),
        ("POST", ["me", "notices", "seen"]) => q.is_none(),
        _ => false,
    }
}

pub fn table_id_ok(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

fn str_in(v: Option<&Value>, set: &[&str]) -> bool {
    v.and_then(Value::as_str).is_some_and(|s| set.contains(&s))
}

fn opt_obj(o: &Map<String, Value>, k: &str) -> Result<Option<Map<String, Value>>, String> {
    match o.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(m)) => Ok(Some(m.clone())),
        Some(_) => Err(format!("{k} is not an object")),
    }
}

/// Absent, null, or an array of strings (what QML calls .indexOf on).
fn strings_ok(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => true,
        Some(Value::Array(a)) => a.iter().all(Value::is_string),
        _ => false,
    }
}

fn cards_ok(v: Option<&Value>, max: usize) -> bool {
    match v {
        None => true,
        Some(Value::Array(a)) => a.len() <= max && a.iter().all(|c| c.as_str().is_some_and(card_ok)),
        _ => false,
    }
}

const SEAT_STATES: [&str; 7] = ["waiting", "active", "folded", "allin", "away", "out", "leaving"];
const STREETS: [&str; 5] = ["preflop", "flop", "turn", "river", "showdown"];
const HOLDEM_STATUS: [&str; 4] = ["lobby", "hand", "pause", "closed"];
const RIG_STATUS: [&str; 4] = ["lobby", "round", "end", "closed"];
const RIG_ROLES: [&str; 4] = ["spinner", "mechanic", "sharp", "railbird"];
const RIG_PHASES: [&str; 7] = ["setup", "turn", "spin", "land", "nudge", "payout", "reveal"];
const BOARD_STATES: [&str; 4] = ["open", "dead", "busted", "wrong"];

fn check_holdem(o: &Map<String, Value>) -> Result<(), String> {
    let seats = o.get("seats").and_then(Value::as_array).ok_or("seats missing")?;
    if seats.len() > 8 {
        return Err("more than 8 seats".into());
    }
    let mut taken = [false; 9];
    for s in seats {
        let s = s.as_object().ok_or("seat not an object")?;
        let n = uint(s.get("seat")).filter(|n| (1..=8).contains(n)).ok_or("seat number")? as usize;
        if std::mem::replace(&mut taken[n], true) {
            return Err("seat twice".into());
        }
        if !mxid_ok(s.get("u")) {
            return Err("seat user".into());
        }
        uint(s.get("stack")).ok_or("seat stack")?;
        if s.contains_key("bet") {
            uint(s.get("bet")).ok_or("seat bet")?;
        }
        if !str_in(s.get("state"), &SEAT_STATES) {
            return Err("seat state".into());
        }
        if s.contains_key("cards") && uint(s.get("cards")).filter(|&c| c <= 2).is_none() {
            return Err("seat cards".into());
        }
        if !cards_ok(s.get("shown"), 2) {
            return Err("shown cards".into());
        }
    }
    if let Some(h) = opt_obj(o, "hand")? {
        uint(h.get("n")).ok_or("hand n")?;
        if !str_in(h.get("street"), &STREETS) {
            return Err("street".into());
        }
        if !cards_ok(h.get("board"), 5) {
            return Err("board".into());
        }
        if let Some(p) = h.get("pots") {
            let p = p.as_array().ok_or("pots")?;
            for pot in p {
                uint(pot.get("amount")).ok_or("pot amount")?;
            }
        }
        if let Some(t) = opt_obj(&h, "to_act")? {
            uint(t.get("seat")).ok_or("to_act seat")?;
            uint(t.get("turn")).ok_or("to_act turn")?;
            if t.get("deadline").is_some_and(|d| !d.is_null()) {
                uint(t.get("deadline")).ok_or("to_act deadline")?;
            }
        }
    }
    if let Some(y) = opt_obj(o, "you")? {
        if !strings_ok(y.get("screens")) {
            return Err("you screens".into());
        }
        uint(y.get("seat")).ok_or("you seat")?;
        if !cards_ok(y.get("cards"), 2) {
            return Err("your cards".into());
        }
        if let Some(l) = opt_obj(&y, "legal")? {
            if let Some(r) = opt_obj(&l, "raise")? {
                let (min, max) = (uint(r.get("min")).ok_or("raise min")?, uint(r.get("max")).ok_or("raise max")?);
                if min > max {
                    return Err("raise min > max".into());
                }
            }
            if let Some(c) = l.get("call") {
                if !c.is_boolean() && uint(Some(c)).is_none() {
                    return Err("call".into());
                }
            }
        }
    }
    Ok(())
}

fn check_rig(o: &Map<String, Value>) -> Result<(), String> {
    let players = o.get("players").and_then(Value::as_array).ok_or("players missing")?;
    if players.len() > 8 {
        return Err("more than 8 players".into());
    }
    for p in players {
        if !mxid_ok(p.get("u")) {
            return Err("player user".into());
        }
        if p.get("role").is_some_and(|r| !r.is_null()) && !str_in(p.get("role"), &RIG_ROLES) {
            return Err("player role".into());
        }
    }
    if let Some(r) = opt_obj(o, "round")? {
        uint(r.get("r")).ok_or("round r")?;
        uint(r.get("spin")).ok_or("round spin")?;
        if !str_in(r.get("phase"), &RIG_PHASES) {
            return Err("phase".into());
        }
        if r.get("dial").is_some_and(|d| !d.is_null()) {
            uint(r.get("dial")).filter(|d| (1..=3).contains(d)).ok_or("dial")?;
        }
        if let Some(reels) = r.get("reels").filter(|x| !x.is_null()) {
            let reels = reels.as_array().ok_or("reels")?;
            if reels.len() != 3 || !reels.iter().all(|x| str_in(Some(x), &RIG_SYMBOLS)) {
                return Err("round reels".into());
            }
        }
        if let Some(b) = r.get("board") {
            for c in b.as_array().ok_or("board")? {
                if !str_in(c.get("card"), &RIG_CARDS) || !str_in(c.get("state"), &BOARD_STATES) {
                    return Err("board card".into());
                }
                // who planted a card is secret until it is busted (or the reveal names it)
                let state = c.get("state").and_then(Value::as_str).unwrap_or("");
                if matches!(state, "open" | "dead") && (c.get("owner").is_some_and(|o| !o.is_null()) || c.get("by").is_some_and(|o| !o.is_null())) {
                    return Err("an owner on an unbusted card".into());
                }
            }
        }
        if let Some(h) = r.get("history") {
            for s in h.as_array().ok_or("history")? {
                let reels = s.get("reels").and_then(Value::as_array).ok_or("reels")?;
                if reels.len() != 3 || !reels.iter().all(|x| str_in(Some(x), &RIG_SYMBOLS)) {
                    return Err("reels".into());
                }
                uint(s.get("payout")).ok_or("payout")?;
                if !strings_ok(s.get("stamps")) {
                    return Err("stamps".into());
                }
                uint(s.get("bet")).filter(|b| (1..=3).contains(b)).ok_or("bet")?;
            }
        }
    }
    if let Some(y) = opt_obj(o, "you")? {
        let role = y.get("role").and_then(Value::as_str).unwrap_or("");
        if let Some(c) = opt_obj(&y, "card")? {
            if !str_in(c.get("card"), &RIG_CARDS) {
                return Err("your card".into());
            }
            if role != "mechanic" {
                return Err("a card on a non-Mechanic".into());
            }
        }
        if let Some(ws) = y.get("whispers") {
            for w in ws.as_array().ok_or("whispers")? {
                if w.get("text").and_then(Value::as_str).is_some_and(|t| t.chars().count() > 60) {
                    return Err("whisper too long".into());
                }
            }
        }
        if let Some(d) = y.get("deal") {
            let d = d.as_array().ok_or("deal")?;
            if d.len() > 2 || !d.iter().all(|x| str_in(Some(x), &RIG_CARDS)) {
                return Err("deal".into());
            }
        }
    }
    Ok(())
}

/// Check a `view` snapshot (the `view` member of a `{"t":"view"}` frame, or a `GET /tables/{id}` answer).
/// Returns `(game, table, vseq)` for a view QML may draw; `Err(why)` for anything malformed (dropped whole).
pub fn check_view(v: &Value) -> Result<(String, String, u64), String> {
    let o = v.as_object().ok_or("view is not an object")?;
    let game = o.get("game").and_then(Value::as_str).filter(|g| GAMES.contains(g)).ok_or("game")?;
    let table = o.get("table").and_then(Value::as_str).filter(|t| table_id_ok(t)).ok_or("table")?;
    let vseq = uint(o.get("vseq")).filter(|&n| n >= 1).ok_or("vseq")?;
    let statuses: &[&str] = if game == "holdem" { &HOLDEM_STATUS } else { &RIG_STATUS };
    if !str_in(o.get("status"), statuses) {
        return Err("status".into());
    }
    // rev 2: every view names its room, its space and the space's owner
    for k in ["room", "space"] {
        if !o.get(k).and_then(Value::as_str).is_some_and(|s| s.starts_with('!') && s.len() <= 255) {
            return Err(format!("{k}"));
        }
    }
    if !o.get("owner").and_then(Value::as_str).is_some_and(|s| !s.is_empty() && s.len() <= 255) {
        return Err("owner".into());
    }
    if o.contains_key("now") {
        uint(o.get("now")).ok_or("now")?;
    }
    if o.contains_key("host") && !mxid_ok(o.get("host")) {
        return Err("host".into());
    }
    match game {
        "holdem" => check_holdem(o)?,
        _ => check_rig(o)?,
    }
    Ok((game.to_owned(), table.to_owned(), vseq))
}

// ---------------------------------------------------------------------------------------------------------------
// §6.3 outgoing frames
// ---------------------------------------------------------------------------------------------------------------

/// Table-level moves carry no `at` (§6.3).
pub fn table_level(game: &str, action: &str) -> bool {
    match game {
        "holdem" => matches!(action, "sit" | "leave" | "deal" | "away" | "kick" | "close" | "react"),
        // The Rig's away / I'm back (contract v1.1e) is table-level too
        _ => matches!(action, "join" | "leave" | "start" | "kick" | "close" | "react" | "away"),
    }
}

/// The `at` a move names, read from the view it was made on: Hold'em `{hand, turn}`, The Rig `{round, spin, phase}`.
/// None when the view has no current turn (then only table-level moves make sense).
pub fn at_for(view: &Value) -> Option<Value> {
    match view.get("game").and_then(Value::as_str)? {
        "holdem" => {
            let h = view.get("hand")?;
            Some(json!({"hand": h.get("n")?, "turn": h.get("to_act")?.get("turn")?}))
        }
        "rig" => {
            let r = view.get("round")?;
            Some(json!({"round": uint(r.get("r"))?, "spin": uint(r.get("spin"))?, "phase": r.get("phase")?.as_str()?}))
        }
        _ => None,
    }
}

/// Build an `act` frame for `a` (`{"do": …, …}`) on the view it was made on. None = refused locally (a turn move
/// with no current turn). The server is still the judge (`stale`, `not-your-turn`, …).
pub fn act_frame(view: &Value, op: &str, a: &Value) -> Option<Value> {
    let game = view.get("game")?.as_str()?;
    let table = view.get("table")?.as_str()?;
    let action = a.get("do")?.as_str()?;
    let mut f = json!({"t": "act", "table": table, "op": op, "a": a});
    if !table_level(game, action) {
        f["at"] = at_for(view)?;
    }
    Some(f)
}

// ---------------------------------------------------------------------------------------------------------------
// §6.6 the card bodies (no amounts, ever)
// ---------------------------------------------------------------------------------------------------------------

/// The `body` other clients show for a game card (§6.6). The invite's `{who}` is the poster's display name.
pub fn post_body(kind: &str, game: &str) -> &'static str {
    match (kind, game) {
        ("invite", "holdem") => "{who} opened a Texas Hold'em table: join from the call.",
        ("result" | "summary", "holdem") => "Texas Hold'em results are in: open this in Tin Hat.",
        ("invite", _) => "A game is on in this call.",
        _ => "Game results are in: open this in Tin Hat.",
    }
}

/// The full card content for `m.room.message` (§6.6, rev 2: the space's owner + the space).
pub fn card_content(kind: &str, game: &str, server: &str, space: &str, table: &str, round: Option<u64>, who: &str) -> Value {
    let body = post_body(kind, game).replace("{who}", who);
    let mut block = json!({"v": 1, "kind": kind, "game": game, "server": server, "space": space, "table": table});
    if let Some(r) = round {
        block["round"] = json!(r);
    }
    json!({"msgtype": "m.text", "body": body, CARD_KEY: block})
}

/// A game card's pointer block, read.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub kind: String,
    pub game: String,
    pub server: String,
    pub space: String,
    pub table: String,
    pub round: Option<u64>,
}

/// Read a card block from a message's content; None when malformed.
pub fn card_of(content: &Value) -> Option<Card> {
    let b = content.get(CARD_KEY)?.as_object()?;
    if b.get("v").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    let kind = b.get("kind")?.as_str().filter(|k| matches!(*k, "invite" | "result" | "summary"))?;
    let game = b.get("game")?.as_str().filter(|g| GAMES.contains(g))?;
    let server = b.get("server")?.as_str().filter(|s| !s.is_empty() && s.len() <= 255)?;
    let space = b.get("space")?.as_str().filter(|s| s.starts_with('!') && s.len() <= 255)?;
    let table = b.get("table")?.as_str().filter(|t| table_id_ok(t))?;
    Some(Card { kind: kind.into(), game: game.into(), server: server.into(), space: space.into(), table: table.into(), round: uint(b.get("round")) })
}

// ---------------------------------------------------------------------------------------------------------------
// testvectors/games/ui.json: the small UI derivations both apps draw the same way (not game rules)
// ---------------------------------------------------------------------------------------------------------------

/// The raise bar's quick amounts (raise TO): `inp` = `{pots: [n], bets: [n], call: n|false, raise: {min, max}|null,
/// f: 0.5|1|"max"}` (ui.json `raise_preset`).
pub fn raise_preset(inp: &Value) -> Option<u64> {
    let r = inp.get("raise").filter(|r| r.is_object())?;
    let (min, max) = (r.get("min")?.as_u64()?, r.get("max")?.as_u64()?);
    if min > max || max > MAX_SAFE_INT {
        return None;
    }
    if inp.get("f").and_then(Value::as_str) == Some("max") {
        return Some(max);
    }
    let f = inp.get("f")?.as_f64().filter(|f| (0.0..=1.0).contains(f))?;
    let sum = |k: &str| inp.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).fold(0u64, |s, x| s.saturating_add(x))).unwrap_or(0);
    let top = inp.get("bets").and_then(Value::as_array).and_then(|a| a.iter().filter_map(Value::as_u64).max()).unwrap_or(0);
    let call = inp.get("call").and_then(Value::as_u64).unwrap_or(0);
    let to = top.saturating_add((f * sum("pots").saturating_add(sum("bets")).saturating_add(call) as f64).floor() as u64);
    Some(to.clamp(min, max))
}

/// [`raise_preset`] read from a Hold'em view.
pub fn raise_preset_view(view: &Value, f: &Value) -> Option<u64> {
    let pots: Vec<Value> = view.pointer("/hand/pots").and_then(Value::as_array).map(|p| p.iter().filter_map(|x| x.get("amount").cloned()).collect()).unwrap_or_default();
    let bets: Vec<Value> = view.get("seats").and_then(Value::as_array).map(|s| s.iter().map(|x| x.get("bet").cloned().unwrap_or(json!(0))).collect()).unwrap_or_default();
    let legal = view.pointer("/you/legal");
    raise_preset(&json!({
        "pots": pots, "bets": bets,
        "call": legal.and_then(|l| l.get("call")).cloned().unwrap_or(json!(0)),
        "raise": legal.and_then(|l| l.get("raise")).cloned().unwrap_or(Value::Null),
        "f": f,
    }))
}

/// Where a Hold'em seat is drawn: slot 0 = bottom centre, clockwise, rotated so my seat is slot 0 (ui.json `seat_slot`).
pub fn seat_slot(seat: u32, my_seat: u32, seats: u32) -> u32 {
    let n = seats.max(1) as i64;
    let base = if my_seat > 0 { my_seat } else { 1 } as i64;
    ((seat as i64 - base) % n + n) as u32 % seats.max(1)
}

/// A ui.json art colour ("#rrggbb" or "rgba(r, g, b, a)") as QML writes it ("#rrggbb" / "#aarrggbb"); None if neither.
pub fn qml_colour(css: &str) -> Option<String> {
    let c = css.trim();
    if c.len() == 7 && c.starts_with('#') && c[1..].bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some(c.to_ascii_lowercase());
    }
    let inner = c.strip_prefix("rgba(")?.strip_suffix(')')?;
    let p: Vec<&str> = inner.split(',').map(str::trim).collect();
    if p.len() != 4 {
        return None;
    }
    let ch = |s: &str| s.parse::<u8>().ok();
    let (r, g, b) = (ch(p[0])?, ch(p[1])?, ch(p[2])?);
    let a: f64 = p[3].parse().ok().filter(|a: &f64| (0.0..=1.0).contains(a))?;
    Some(format!("#{:02x}{:02x}{:02x}{:02x}", (a * 255.0).round() as u8, r, g, b))
}

/// ui.json `art` + `symbol_colours` as one QML-ready map (unknown values dropped).
pub fn art_for_qml(ui: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for group in ["art", "symbol_colours"] {
        for (k, v) in ui.get(group).and_then(Value::as_object).into_iter().flatten() {
            if let Some(c) = v.as_str().and_then(qml_colour) {
                let key = if group == "art" { k.clone() } else { format!("sym_{k}") };
                out.insert(key, Value::String(c));
            }
        }
    }
    Value::Object(out)
}

/// The call strip's chip for a table (ui.json `chip_label`; words.json `chip.*`).
/// A game chat card's action (spec §4.5 + the PM's 15:25 answers; the web mirrors it): `"open"` | `"join_call"` | `""`.
/// Only an invite has one (a result / summary card is its record). `status` = the table's listing status (None = not
/// listed any more). People not in the call see a LIVE table's status only and get "Join the call" (the server refuses
/// live views outside the call); an ENDED table opens (its results view) only in the call, outside it the card says
/// "Table over" with no action (the chat's results card carries the results); closed / unlisted: none.
pub fn card_action(kind: &str, status: Option<&str>, in_call: bool) -> &'static str {
    if kind != "invite" {
        return "";
    }
    match status {
        None | Some("closed") => "",
        Some("end") if in_call => "open",
        Some("end") => "",
        Some(_) if in_call => "open",
        Some(_) => "join_call",
    }
}

/// The statuses that get a chip: an `end` table (results on screen until it closes) or a closed one
/// gets none, so an ended table never offers Join.
pub const CHIP_STATUSES: [&str; 4] = ["lobby", "hand", "pause", "round"];

/// ui.json `chip_label`; "" = no chip for this table (the vectors' `expect: null`).
pub fn chip_label(words: &Value, table: &Value) -> String {
    if !table.get("status").and_then(Value::as_str).is_some_and(|s| CHIP_STATUSES.contains(&s)) {
        return String::new();
    }
    let w = &words["chip"];
    let n = |k: &str| table.get(k).and_then(Value::as_u64).unwrap_or(0).to_string();
    let (tpl, val) = match (table.get("game").and_then(Value::as_str), table.get("status").and_then(Value::as_str)) {
        (Some("rig"), Some("round")) => ("rig", n("round")),
        (Some("rig"), _) => ("rig_lobby", n("seated")),
        _ => ("holdem", n("seated")),
    };
    w.get(tpl).and_then(Value::as_str).unwrap_or("").replace("{n}", &val)
}

#[cfg(test)]
mod tests {
    use super::*;



    const UI: &str = include_str!("testvectors/games/ui.json");
    const WORDS: &str = include_str!("testvectors/games/words.json");

    /// The card action rule: failing-first against the rule it replaces (Open only in the call,
    /// nothing outside it), which gets the 3 live-outside-the-call cases wrong.
    #[test]
    fn card_actions_follow_the_spec() {
        let cases: [(&str, Option<&str>, bool, &str); 12] = [
            ("invite", Some("lobby"), true, "open"),
            ("invite", Some("hand"), true, "open"),
            ("invite", Some("round"), true, "open"),
            ("invite", Some("lobby"), false, "join_call"),
            ("invite", Some("hand"), false, "join_call"),
            ("invite", Some("pause"), false, "join_call"),
            ("invite", Some("end"), false, ""),
            ("invite", Some("end"), true, "open"),
            ("invite", Some("closed"), true, ""),
            ("invite", None, false, ""),
            ("result", Some("end"), true, ""),
            ("summary", None, false, ""),
        ];
        let old = |kind: &str, status: Option<&str>, in_call: bool| -> &'static str {
            if kind == "invite" && in_call && status.is_some_and(|s| s != "closed") { "open" } else { "" }
        };
        let mut old_wrong = 0;
        for (kind, status, in_call, want) in cases {
            assert_eq!(card_action(kind, status, in_call), want, "{kind} {status:?} in_call={in_call}");
            if old(kind, status, in_call) != want {
                old_wrong += 1;
            }
        }
        assert_eq!(old_wrong, 3, "the replaced rule must fail exactly the 3 live-outside-the-call cases");
    }

    #[test]
    fn ui_vectors() {
        let ui: Value = serde_json::from_str(UI).unwrap();
        let words: Value = serde_json::from_str(WORDS).unwrap();
        for c in ui["card_face"]["cases"].as_array().unwrap() {
            assert_eq!(card_face(c["card"].as_str().unwrap()).unwrap_or(Value::Null), c["expect"], "card_face {}", c["card"]);
        }
        for c in ui["raise_preset"]["cases"].as_array().unwrap() {
            assert_eq!(raise_preset(&c["in"]).map(Value::from).unwrap_or(Value::Null), c["expect"], "raise_preset {}", c["id"]);
        }
        for c in ui["seat_slot"]["cases"].as_array().unwrap() {
            let g = |k: &str| c[k].as_u64().unwrap() as u32;
            assert_eq!(seat_slot(g("seat"), g("my_seat"), g("seats")) as u64, c["expect"].as_u64().unwrap(), "seat_slot {c}");
        }
        for c in ui["chip_label"]["cases"].as_array().unwrap() {
            assert_eq!(chip_label(&words, &c["table"]), c["expect"].as_str().unwrap_or(""), "chip_label {c}");
        }
        // the control: the old rule (a chip for every listed table) fails the no-chip cases
        let every = |t: &Value| -> String {
            let n = |k: &str| t.get(k).and_then(Value::as_u64).unwrap_or(0).to_string();
            match (t["game"].as_str(), t["status"].as_str()) {
                (Some("rig"), Some("round")) => words["chip"]["rig"].as_str().unwrap().replace("{n}", &n("round")),
                (Some("rig"), _) => words["chip"]["rig_lobby"].as_str().unwrap().replace("{n}", &n("seated")),
                _ => words["chip"]["holdem"].as_str().unwrap().replace("{n}", &n("seated")),
            }
        };
        let nulls: Vec<&Value> = ui["chip_label"]["cases"].as_array().unwrap().iter().filter(|c| c["expect"].is_null()).collect();
        assert!(nulls.len() >= 3);
        for c in nulls {
            assert_ne!(every(&c["table"]), "", "the planted every-table rule must fail {c}");
        }
        // the art: every colour converts (QML can't read CSS rgba())
        let art = art_for_qml(&ui);
        assert_eq!(art.as_object().unwrap().len(), ui["art"].as_object().unwrap().len() - 1 + ui["symbol_colours"].as_object().unwrap().len());
        assert_eq!(qml_colour("rgba(255, 255, 255, 0.10)").unwrap(), "#1affffff");
        assert_eq!(qml_colour("#1D5A4A").unwrap(), "#1d5a4a");
        assert!(qml_colour("rgba(1,2,3)").is_none() && qml_colour("red").is_none() && qml_colour("rgba(1,2,3,2)").is_none());
        // the view reader agrees with the plain one on the spec's example
        assert_eq!(raise_preset_view(&holdem_example(), &json!(0.5)), Some(32));
        assert_eq!(raise_preset_view(&holdem_example(), &json!("max")), Some(40));
    }

    /// The shared files are the web's byte copies (skipped until the web has them).
    #[test]
    fn ui_and_words_are_the_webs() {
        let dir = std::env::var_os("TINHAT_WEB_DIR")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("tinhat-web")));
        let Some(dir) = dir else { return };
        for (name, ours) in [("ui.json", UI), ("words.json", WORDS)] {
            let mut found = false;
            for rel in ["src/features/games/vectors", "testvectors/games"] {
                if let Ok(theirs) = std::fs::read_to_string(dir.join(rel).join(name)) {
                    assert!(ours == theirs, "{name} differs from the web's {rel}/{name}");
                    found = true;
                    break;
                }
            }
            if !found {
                eprintln!("SKIPPED: no web copy of {name} under {}", dir.display());
            }
        }
    }


    #[test]
    fn op_ids() {
        let a = new_op_id();
        assert!(op_id_ok(&a), "{a}");
        assert_ne!(a, new_op_id());
        assert!(!op_id_ok("short") && !op_id_ok("has a space in it!!") && !op_id_ok(&"x".repeat(33)));
    }

    #[test]
    fn newest_vseq() {
        let mut n = Newest::default();
        assert!(n.accept("t1", 5));
        assert!(!n.accept("t1", 5), "a repeat is dropped");
        assert!(!n.accept("t1", 4), "an older one is dropped");
        assert!(n.accept("t1", 6) && n.accept("t2", 1));
        n.forget("t1");
        assert!(n.accept("t1", 1));
    }

    #[test]
    fn frames() {
        assert!(frame_ok(r#"{"t":"hello","v":1}"#).is_some());
        assert!(frame_ok(r#"{"v":1}"#).is_none(), "no t");
        assert!(frame_ok(r#"{"t":1}"#).is_none(), "t not a string");
        assert!(frame_ok(&format!(r#"{{"t":"x","pad":"{}"}}"#, "a".repeat(MAX_FRAME))).is_none(), "too big");
        assert!(frame_ok(r#"{"t":"x","n":9007199254740992}"#).is_none(), "unsafe integer");
        assert!(frame_ok(r#"{"t":"x","n":9007199254740991}"#).is_some());
        let deep = format!(r#"{{"t":"x","d":{}{}}}"#, "[".repeat(40), "]".repeat(40));
        assert!(frame_ok(&deep).is_none(), "too deep");
        assert!(frame_ok(r#"{"t":"x","s":"\ud800"}"#).is_none(), "lone surrogate");
    }

    #[test]
    fn cards() {
        assert_eq!(card_face("Td").unwrap(), json!({"rank": "10", "suit": "♦", "red": true}));
        assert_eq!(card_face("As").unwrap(), json!({"rank": "A", "suit": "♠", "red": false}));
        assert!(card_face("1s").is_none() && card_face("Ax").is_none() && card_face("A").is_none());
    }

    fn holdem_example() -> Value {
        // §4.3's example view, as the spec writes it.
        json!({"game":"holdem","table":"tH3k9","room":"!r:x","space":"!S:x","owner":"x","vseq":41,"now":1763600000123u64,"status":"hand","host":"@a:x",
         "opts":{"sb":1,"bb":2,"seats":8,"turn_s":30},
         "seats":[{"seat":1,"u":"@a:x","stack":38,"bet":2,"state":"active","cards":2},
                  {"seat":4,"u":"@b:y","stack":0,"bet":10,"state":"allin","cards":2}],
         "hand":{"n":17,"button":4,"street":"flop","board":["2c","9d","Ts"],
                 "pots":[{"amount":24,"eligible":[1,4]}],
                 "to_act":{"seat":1,"turn":5,"deadline":1763600030123u64},
                 "last":{"seat":4,"a":"allin","to":10}},
         "you":{"seat":1,"cards":["As","Kd"],"legal":{"fold":true,"check":false,"call":8,"raise":{"min":18,"max":40}},
                "screens":["desktop","pad"],"at_cap":false}})
    }

    fn rig_example() -> Value {
        // §5.3's example view (a Mechanic mid-round).
        json!({"game":"rig","table":"tR7q2","room":"!r:x","space":"!S:x","owner":"x","vseq":88,"now":1763600000123u64,"status":"round","host":"@a:x",
         "opts":{"preset":"full","bankroll":6,"patch":true},
         "players":[{"u":"@a:x","role":"spinner"},{"u":"@b:y","role":"mechanic"},{"u":"@c:x","role":"mechanic"},
                    {"u":"@d:x","role":"sharp"}],
         "round":{"r":2,"spinner":"@a:x","phase":"turn","spin":4,"spins":10,"deadline":1763600020123u64,
                  "seed_commit":"9f2c","bankroll":{"start":6,"now":14},"dial":2,"calls_left":2,"audit_used":false,
                  "walk_from":6,"peek_lock_after":4,
                  "board":[{"card":"moon-monday","state":"open"},{"card":"skim","state":"dead"},
                           {"card":"rebound","state":"busted","by":"@c:x","spin":3},{"card":"hot-hat","state":"wrong"}],
                  "refilled":[{"spin":3}],
                  "history":[{"spin":1,"bet":1,"reels":["eye","moon","dish"],"payout":0,"stamps":[]},
                             {"spin":2,"bet":3,"reels":["dish","dish","eye"],"payout":3,"stamps":["nudged"]},
                             {"spin":3,"bet":2,"reels":["hat","eye","hat"],"payout":8,"stamps":["called-right"]}]},
         "you":{"role":"mechanic","card":{"card":"moon-monday","bite":2},"mutes_left":1,"armed":false,"rake":3,
                "nudge_left":1,"whisper_used":false,"peek":{"placed":false,"locks_after":4},"market":{"side":"up","stake":1},
                "screens":["desktop","pad"]}})
    }

    #[test]
    fn spec_views_pass() {
        assert_eq!(check_view(&holdem_example()).unwrap(), ("holdem".into(), "tH3k9".into(), 41));
        assert_eq!(check_view(&rig_example()).unwrap(), ("rig".into(), "tR7q2".into(), 88));
    }

    /// Every planted defect is refused (each one alone, on an otherwise good view).
    #[test]
    fn malformed_views_refused() {
        let plants: Vec<(&str, Box<dyn Fn(&mut Value)>)> = vec![
            ("unknown game", Box::new(|v| v["game"] = json!("bingo"))),
            ("table id with a slash", Box::new(|v| v["table"] = json!("../x"))),
            ("vseq a string", Box::new(|v| v["vseq"] = json!("41"))),
            ("vseq negative", Box::new(|v| v["vseq"] = json!(-1))),
            ("no status", Box::new(|v| { v.as_object_mut().unwrap().remove("status"); })),
            ("seat twice", Box::new(|v| v["seats"][1]["seat"] = json!(1))),
            ("seat 9", Box::new(|v| v["seats"][1]["seat"] = json!(9))),
            ("negative stack", Box::new(|v| v["seats"][0]["stack"] = json!(-5))),
            ("fractional stack", Box::new(|v| v["seats"][0]["stack"] = json!(2.5))),
            ("bad seat state", Box::new(|v| v["seats"][0]["state"] = json!("sleeping"))),
            ("bad user", Box::new(|v| v["seats"][0]["u"] = json!("bob"))),
            ("three hole cards", Box::new(|v| v["you"]["cards"] = json!(["As", "Kd", "Qh"]))),
            ("bad card", Box::new(|v| v["hand"]["board"][0] = json!("1c"))),
            ("six board cards", Box::new(|v| v["hand"]["board"] = json!(["2c","3c","4c","5c","6c","7c"]))),
            ("raise min > max", Box::new(|v| v["you"]["legal"]["raise"] = json!({"min": 50, "max": 40}))),
            ("street unknown", Box::new(|v| v["hand"]["street"] = json!("fifth"))),
            ("hand not an object", Box::new(|v| v["hand"] = json!([1]))),
            ("screens an object", Box::new(|v| v["you"]["screens"] = json!({}))),
        ];
        for (why, plant) in plants {
            let mut v = holdem_example();
            plant(&mut v);
            assert!(check_view(&v).is_err(), "holdem plant accepted: {why}");
        }
        let rig_plants: Vec<(&str, Box<dyn Fn(&mut Value)>)> = vec![
            ("unknown card", Box::new(|v| v["round"]["board"][0]["card"] = json!("lucky-seven"))),
            ("bad board state", Box::new(|v| v["round"]["board"][0]["state"] = json!("maybe"))),
            ("two reels", Box::new(|v| v["round"]["history"][0]["reels"] = json!(["eye", "moon"]))),
            ("unknown symbol", Box::new(|v| v["round"]["history"][0]["reels"][0] = json!("cherry"))),
            ("bet 4", Box::new(|v| v["round"]["history"][0]["bet"] = json!(4))),
            ("stamps an object", Box::new(|v| v["round"]["history"][0]["stamps"] = json!({}))),
            ("bad phase", Box::new(|v| v["round"]["phase"] = json!("bonus"))),
            ("bad role", Box::new(|v| v["players"][0]["role"] = json!("dealer"))),
            ("no players", Box::new(|v| { v.as_object_mut().unwrap().remove("players"); })),
            ("your card unknown", Box::new(|v| v["you"]["card"]["card"] = json!("joker"))),
        ];
        for (why, plant) in rig_plants {
            let mut v = rig_example();
            plant(&mut v);
            assert!(check_view(&v).is_err(), "rig plant accepted: {why}");
        }
    }

    /// S1's contract fixtures (testvectors/games/views = the server repo's test/vectors/play/views, byte copies):
    /// every view of every step of both games is drawn; every malformed view / card is refused.
    #[test]
    fn contract_views() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/testvectors/games/views");
        let mut seen = 0;
        for game in ["holdem", "rig"] {
            let mut files: Vec<_> = std::fs::read_dir(dir.join(game)).unwrap().map(|e| e.unwrap().path()).collect();
            files.sort();
            assert!(files.len() >= 9, "{game}: {} steps", files.len());
            for f in files {
                let step: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
                for (who, v) in step["views"].as_object().unwrap() {
                    if let Err(why) = check_view(v) {
                        panic!("{} {who}: refused a contract view: {why}", f.display());
                    }
                    seen += 1;
                }
            }
        }
        assert!(seen > 40, "{seen} views");
        let bad: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("malformed.json")).unwrap()).unwrap();
        let mut refused = 0;
        for c in bad["cases"].as_array().unwrap() {
            match c["def"].as_str().unwrap() {
                "holdemView" | "rigView" => {
                    assert!(check_view(&c["value"]).is_err(), "accepted malformed {}", c["id"]);
                    refused += 1;
                }
                "chatCard" => {
                    assert!(card_of(&json!({CARD_KEY: c["value"]})).is_none(), "accepted malformed {}", c["id"]);
                    refused += 1;
                }
                _ => {}
            }
        }
        for c in bad["cases"].as_array().unwrap() {
            if c["def"] == "serverFrame" {
                assert!(!server_frame_ok(&c["value"]), "accepted malformed frame {}", c["id"]);
                refused += 1;
            }
        }
        for c in bad["tolerated"].as_array().unwrap() {
            match c["def"].as_str().unwrap() {
                "holdemView" | "rigView" => assert!(check_view(&c["value"]).is_ok(), "refused tolerated {}", c["id"]),
                "serverFrame" => assert!(server_frame_ok(&c["value"]), "refused tolerated {}", c["id"]),
                other => panic!("tolerated def {other}"),
            }
        }
        // every server frame example of the contract passes
        let frames: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("frames.json")).unwrap()).unwrap();
        let mut n = 0;
        let mut walk = vec![&frames["server"]];
        while let Some(v) = walk.pop() {
            match v {
                Value::Object(o) if o.get("t").is_some_and(Value::is_string) && o.get("t") != Some(&json!("act")) => {
                    if !["auth", "sub", "unsub", "open", "close", "ping"].contains(&o["t"].as_str().unwrap()) {
                        assert!(server_frame_ok(v), "refused contract frame {v}");
                        n += 1;
                    }
                }
                Value::Object(o) => walk.extend(o.values()),
                Value::Array(a) => walk.extend(a.iter()),
                _ => {}
            }
        }
        assert!(n >= 5, "{n} server frames");
        assert!(refused >= 20, "{refused}");
    }

    /// Every move of the contract (schema.json x-moves.shapes: [name, required, optional, carries at]) gets `at`
    /// exactly when the contract says, built on real contract views.
    #[test]
    fn contract_move_shapes() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/testvectors/games/views");
        let schema: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("schema.json")).unwrap()).unwrap();
        let view_of = |f: &str| -> Value {
            let step: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(f)).unwrap()).unwrap();
            step["views"].as_object().unwrap().values().find(|v| v.get("you").is_some()).cloned().unwrap()
        };
        let views = [("holdem", view_of("holdem/04-flop.json")), ("rig", view_of("rig/04-turn-full.json"))];
        let mut n = 0;
        for (game, view) in &views {
            for m in schema["x-moves"]["shapes"][*game].as_array().unwrap() {
                let name = m[0].as_str().unwrap();
                let action = name.split('-').next().unwrap(); // market-pass, whisper-text
                let mut a = json!({"do": action});
                for k in m[1].as_array().unwrap() {
                    a[k.as_str().unwrap()] = json!(1);
                }
                let f = act_frame(view, "op_aaaaaaaaaaaaaaaa", &a).unwrap_or_else(|| panic!("{game} {name}: no frame"));
                assert_eq!(f.get("at").is_some(), m[3].as_bool().unwrap(), "{game} {name}: at");
                n += 1;
            }
        }
        assert!(n >= 25, "{n}");
    }

    #[test]
    fn act_frames() {
        let v = holdem_example();
        let f = act_frame(&v, "op_aaaaaaaaaaaaaaaa", &json!({"do": "raise", "to": 18})).unwrap();
        assert_eq!(f, json!({"t":"act","table":"tH3k9","op":"op_aaaaaaaaaaaaaaaa","at":{"hand":17,"turn":5},"a":{"do":"raise","to":18}}));
        let f = act_frame(&v, "op_aaaaaaaaaaaaaaaa", &json!({"do": "leave"})).unwrap();
        assert!(f.get("at").is_none(), "table-level moves carry no at");
        let mut lobby = v.clone();
        lobby.as_object_mut().unwrap().remove("hand");
        assert!(act_frame(&lobby, "op_aaaaaaaaaaaaaaaa", &json!({"do": "fold"})).is_none());
        let r = rig_example();
        let f = act_frame(&r, "op_bbbbbbbbbbbbbbbb", &json!({"do": "spin"})).unwrap();
        assert_eq!(f["at"], json!({"round": 2, "spin": 4, "phase": "turn"}));
    }

    #[test]
    fn cards_in_chat() {
        let c = card_content("invite", "holdem", "lab.test", "!S", "tH3k9", None, "Sam");
        assert_eq!(c["body"], "Sam opened a Texas Hold'em table: join from the call.");
        let got = card_of(&c).unwrap();
        assert_eq!((got.kind.as_str(), got.game.as_str(), got.server.as_str(), got.space.as_str(), got.table.as_str(), got.round), ("invite", "holdem", "lab.test", "!S", "tH3k9", None));
        let r = card_content("result", "rig", "lab.test", "!S", "tR7q2", Some(3), "Sam");
        assert_eq!(r["body"], "Game results are in: open this in Tin Hat.");
        assert_eq!(card_of(&r).unwrap().round, Some(3));
        // No amounts in any body.
        for k in ["invite", "result", "summary"] {
            for g in GAMES {
                assert!(!post_body(k, g).chars().any(|c| c.is_ascii_digit()), "{k}/{g}");
            }
        }
        for plant in [json!({"table": "a/b"}), json!({"v": 2}), json!({"space": 5}), json!({"space": "S"}), json!({"kind": "bribe"}), json!({"server": ""})] {
            let mut bad = r.clone();
            for (k, v) in plant.as_object().unwrap() {
                bad[CARD_KEY][k] = v.clone();
            }
            assert!(card_of(&bad).is_none(), "accepted {plant}");
        }
    }


    #[test]
    fn r2_routes_are_allow_listed() {
        for (m, p) in [
            ("POST", "/side/tune"), ("POST", "/side/tune/sRab12CD34"), ("GET", "/admin/spaces/%21S%3Alab.test"),
            ("POST", "/admin/spaces/%21S%3Alab.test/wallets"), ("PUT", "/admin/spaces/%21S%3Alab.test/economy"),
            ("GET", "/admin/spaces/%21S%3Alab.test/log"), ("GET", "/admin/spaces/%21S%3Alab.test/log?before=812&limit=50"),
            // what QML's encodeURIComponent("!S:lab.test") really produces
            ("GET", "/admin/spaces/!S%3Alab.test"), ("PUT", "/admin/spaces/!S%3Alab.test/economy"),
            ("POST", "/me/notices/seen"),
        ] {
            assert!(r2_route_ok(m, p), "{m} {p}");
        }
        for (m, p) in [
            ("GET", "/side/tune"), ("DELETE", "/admin/spaces/%21S%3Alab.test"), ("POST", "/tables"), ("GET", "/me"),
            ("GET", "/admin/spaces/%21S%3Alab.test/../../me"), ("GET", "/admin/spaces/a%2Fb"), ("GET", "/admin/spaces/"),
            ("GET", "/admin/spaces/%21S/log?limit=50&x=1"), ("GET", "/admin/spaces/%21S/log?limit=-1"),
            ("POST", "/side/tune/../x"), ("POST", "/side/tune/xx12"), ("POST", "side/tune"), ("POST", "/side/tune?x=1"),
            ("GET", "https://evil.test/admin/spaces/x"),
            // encoded dot segments (URL parsing turns them into ".."), a lone dot, an encoded NUL
            ("POST", "/admin/spaces/%2e%2e/wallets"), ("POST", "/admin/spaces/.%2e/wallets"), ("POST", "/admin/spaces/%2E./wallets"),
            ("GET", "/admin/spaces/."), ("GET", "/admin/spaces/%2e"), ("GET", "/admin/spaces/a%00b"),
        ] {
            assert!(!r2_route_ok(m, p), "{m} {p} must be refused");
        }
    }

    #[test]
    fn r2_react_frames() {
        let ok = json!({"t": "react", "table": "tH3k9aaaaaaaaaaaaaa", "u": "@ana:x.test", "seat": 1, "r": "toss", "to": 4, "at": 1});
        assert!(server_frame_ok(&ok));
        for bad in [json!({"r": "slap"}), json!({"to": "everyone"}), json!({"u": "ana"}), json!({"seat": -1}), json!({"table": "t h"})] {
            let mut f = ok.clone();
            for (k, v) in bad.as_object().unwrap() {
                f[k] = v.clone();
            }
            assert!(!server_frame_ok(&f), "{f}");
        }
        let mut w = ok.clone();
        w["seat"] = Value::Null;
        w["to"] = json!("pot");
        assert!(server_frame_ok(&w), "a watcher's reaction to the pot");
        // a reaction is table-level: no `at`, in both games
        let view = json!({"game": "holdem", "table": "tH3k9aaaaaaaaaaaaaa", "hand": {"n": 3, "to_act": {"seat": 1, "turn": 2}}});
        let f = act_frame(&view, "op1", &json!({"do": "react", "r": "clap"})).unwrap();
        assert!(f.get("at").is_none());
        assert!(table_level("rig", "react"));
        assert!(CODES.contains(&"cooldown"));
    }
}
