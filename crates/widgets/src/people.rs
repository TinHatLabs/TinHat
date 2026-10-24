//! people — who is listed where, and in which order (contract WIDGETS.md §10.6; vectors testvectors/widgets/people/*):
//! the Call stats rows (their order, each person's band, the summary's worst person, the compact ↓ ↑ label) and Who's
//! around's groups (Playing, In a call, Online, Away, the Small row). A line-for-line port of the web's
//! `src/widgets/people.ts` (v1.3, web 3493c72e): strings compare in code-point order (Rust's `str` order), case changes
//! and trims are ASCII-only, sorts are stable like `Array.prototype.sort`.

use std::cmp::Ordering;
use std::collections::HashSet;

use serde_json::{json, Map, Value};

use crate::history::{is_recorder_user, recorder_input};
use crate::quality::{cq_band, fmt_rate, worst_band, Band};

/// ASCII letters lower-cased (A–Z → a–z); every other character unchanged.
pub fn ascii_lower(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_uppercase() { c.to_ascii_lowercase() } else { c }).collect()
}

/// ASCII whitespace (U+0009–U+000D, U+0020) trimmed from both ends.
pub fn ascii_trim(s: &str) -> &str {
    s.trim_matches(|c: char| c == ' ' || ('\u{9}'..='\u{d}').contains(&c))
}

/// Row order everywhere a list of people is shown: the name with ASCII letters lower-cased, then the name as written,
/// then the id — each in code-point order.
pub fn cmp_people(a_id: &str, a_name: &str, b_id: &str, b_name: &str) -> Ordering {
    ascii_lower(a_name)
        .cmp(&ascii_lower(b_name))
        .then_with(|| a_name.cmp(b_name))
        .then_with(|| a_id.cmp(b_id))
}

/// Call stats "This week" rows (WIDGETS-UI.md §5; 2026-09-27: no per-person call time): the friends by `calls`
/// (most first), then `cmp_people` on the name the row shows — so the order gives no time away. The web's
/// `features/widgets/callstats/order.ts` friendsByCalls. Input (user, calls, name); answers the users in that order.
pub fn friends_by_calls(items: &[(String, i64, String)]) -> Vec<String> {
    let mut v: Vec<&(String, i64, String)> = items.iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| cmp_people(&a.0, &a.2, &b.0, &b.2)));
    v.into_iter().map(|x| x.0.clone()).collect()
}

// ---- Call stats -----------------------------------------------------------------------------------------------------

/// One person in the call (the LiveKit identity, unique per device).
#[derive(Clone, Debug, Default)]
pub struct CallPerson {
    pub id: String,
    pub name: String,
    /// LiveKit ConnectionQuality ("excellent" | "good" | "poor" | "lost" | anything else = unknown)
    pub cq: Option<String>,
    /// the incoming lane's band (worst of the received tracks) and the outgoing lane's (their last tinhat.rxq q)
    pub in_band: Option<Band>,
    pub out_band: Option<Band>,
}

impl CallPerson {
    /// From a JSON object the way the reference reads it (`cq ?? null`, bands only "good" | "ok" | "poor").
    pub fn from_json(v: &Value) -> CallPerson {
        let band = |k: &str| v.get(k).and_then(Value::as_str).and_then(Band::parse);
        CallPerson {
            id: v.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
            name: v.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
            cq: v.get("cq").and_then(Value::as_str).map(str::to_string),
            in_band: band("inBand"),
            out_band: band("outBand"),
        }
    }
}

/// alone: nobody else in the call · measuring: nobody has a band yet · good: every known band is good · worst: `id`
/// has the worst band (ok | poor).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallSummary {
    Alone,
    Measuring,
    Good,
    Worst { id: String, band: Band },
}

#[derive(Clone, Debug)]
pub struct CallPeople {
    /// the ids in row order
    pub order: Vec<String>,
    /// each row's band (row order): worst_band(cq_band(cq), in_band, out_band) — the row's dot uses cq_band alone
    pub bands: Vec<Option<Band>>,
    pub summary: CallSummary,
}

fn rank(b: Band) -> u8 {
    match b {
        Band::Good => 0,
        Band::Ok => 1,
        Band::Poor => 2,
    }
}

/// The Call stats rows: sorted by cmp_people; each person's band; the summary — the FIRST person in row order with the
/// worst band when that band is ok or poor.
pub fn call_people(people: &[CallPerson]) -> CallPeople {
    let mut rows: Vec<&CallPerson> = people.iter().collect();
    rows.sort_by(|a, b| cmp_people(&a.id, &a.name, &b.id, &b.name));
    let bands: Vec<Option<Band>> =
        rows.iter().map(|p| worst_band([cq_band(p.cq.as_deref()), p.in_band, p.out_band])).collect();
    let mut worst: Option<usize> = None;
    for (i, b) in bands.iter().enumerate() {
        if let Some(b) = b {
            if worst.map_or(true, |w| rank(*b) > rank(bands[w].unwrap())) {
                worst = Some(i);
            }
        }
    }
    let summary = if rows.is_empty() {
        CallSummary::Alone
    } else {
        match worst {
            None => CallSummary::Measuring,
            Some(w) if bands[w] == Some(Band::Good) => CallSummary::Good,
            Some(w) => CallSummary::Worst { id: rows[w].id.clone(), band: bands[w].unwrap() },
        }
    };
    CallPeople { order: rows.iter().map(|p| p.id.clone()).collect(), bands, summary }
}

impl CallPeople {
    pub fn to_json(&self) -> Value {
        let summary = match &self.summary {
            CallSummary::Alone => json!({ "kind": "alone" }),
            CallSummary::Measuring => json!({ "kind": "measuring" }),
            CallSummary::Good => json!({ "kind": "good" }),
            CallSummary::Worst { id, band } => json!({ "kind": "worst", "id": id, "band": band.as_str() }),
        };
        json!({
            "order": self.order,
            "bands": self.bands.iter().map(|b| b.map_or(Value::Null, |b| Value::String(b.as_str().into()))).collect::<Vec<_>>(),
            "summary": summary,
        })
    }
}

/// The collapsed / Small summary's rates: "↓ 2.4 ↑ 1.1 Mb/s" (one unit when both rates use the same one), else
/// "↓ 640 kb/s ↑ 1.1 Mb/s" (fmt_rate each).
pub fn fmt_rate_pair(down: f64, up: f64) -> String {
    let a = fmt_rate(down);
    let b = fmt_rate(up);
    let (an, ua) = a.rsplit_once(' ').unwrap_or(("", a.as_str()));
    let (bn, ub) = b.rsplit_once(' ').unwrap_or(("", b.as_str()));
    if ua == ub {
        format!("↓ {an} ↑ {bn} {ua}")
    } else {
        format!("↓ {a} ↑ {b}")
    }
}

// ---- Who's around ---------------------------------------------------------------------------------------------------

/// One person Who's around may list (the Reach state: "online" | "away" | "call"; anything else is not listed).
#[derive(Clone, Debug, Default)]
pub struct AroundPerson {
    pub id: String,
    pub name: String,
    pub state: String,
    /// the call's room when the service tells us (you share that room), else empty
    pub room: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AroundRoom {
    pub room: String,
    /// ids in cmp_people order
    pub people: Vec<String>,
    /// Playing only: the game (ASCII-trimmed, non-empty) and the emoji (ASCII-trimmed, may be "")
    pub game: String,
    pub emoji: String,
}

#[derive(Clone, Debug, Default)]
pub struct AroundGroups {
    pub playing: Vec<AroundRoom>,
    pub in_call: Vec<AroundRoom>,
    /// in a call in a room you are not in (no room from the service)
    pub call_elsewhere: Vec<String>,
    pub online: Vec<String>,
    pub away: Vec<String>,
    /// everyone shown (the groups' people added up)
    pub total: usize,
    /// the Small / collapsed row: Playing rooms' people, In a call rooms' people, call elsewhere, Online, Away
    pub summary: Vec<String>,
}

/// Who's around's groups from the reference's input shape: `{me, people: [{id, name, state, room}], activities:
/// {room: {game, emoji}}, showAway}`. Me and the recorder (the account the server announces) are left out; a person in a call with a room goes
/// to that room's row — Playing when the room's activity has a game, else In a call; rooms ordered by their number of
/// people (most first), then room id; every list of people in cmp_people order; a later duplicate of an id is ignored.
pub fn group_around(input: &Value) -> AroundGroups {
    let me = input.get("me").and_then(Value::as_str).unwrap_or("");
    let show_away = input.get("showAway").and_then(Value::as_bool).unwrap_or(false);
    // the recorder (never listed): the account the server announces; null or absent = none
    let recorder = recorder_input(input);
    let empty = Map::new();
    let activities = input.get("activities").and_then(Value::as_object).unwrap_or(&empty);
    let mut seen: HashSet<String> = HashSet::new();
    // rooms in first-seen order (the reference's Map), then sorted
    let mut rooms: Vec<(String, Vec<AroundPerson>)> = Vec::new();
    let mut elsewhere: Vec<AroundPerson> = Vec::new();
    let mut online: Vec<AroundPerson> = Vec::new();
    let mut away: Vec<AroundPerson> = Vec::new();
    for p in input.get("people").and_then(Value::as_array).map(|a| a.as_slice()).unwrap_or(&[]) {
        let Some(id) = p.get("id").and_then(Value::as_str) else { continue };
        if id.is_empty() || seen.contains(id) {
            continue;
        }
        seen.insert(id.to_string());
        if id == me || is_recorder_user(id, recorder.as_ref().map(|r| r.as_deref())) {
            continue;
        }
        let who = AroundPerson {
            id: id.to_string(),
            name: p.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
            state: p.get("state").and_then(Value::as_str).unwrap_or("").to_string(),
            room: p.get("room").and_then(Value::as_str).unwrap_or("").to_string(),
        };
        match who.state.as_str() {
            "call" => {
                if who.room.is_empty() {
                    elsewhere.push(who);
                } else if let Some(r) = rooms.iter_mut().find(|(r, _)| *r == who.room) {
                    r.1.push(who);
                } else {
                    rooms.push((who.room.clone(), vec![who]));
                }
            }
            "online" => online.push(who),
            "away" => away.push(who),
            _ => {}
        }
    }
    let ids = |l: &[AroundPerson]| -> Vec<String> {
        let mut v: Vec<&AroundPerson> = l.iter().collect();
        v.sort_by(|a, b| cmp_people(&a.id, &a.name, &b.id, &b.name));
        v.iter().map(|p| p.id.clone()).collect()
    };
    rooms.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));
    let mut g = AroundGroups::default();
    for (room, people) in &rooms {
        let act = activities.get(room.as_str());
        let field = |k: &str| act.and_then(|a| a.get(k)).and_then(Value::as_str).map(|s| ascii_trim(s).to_string());
        let game = field("game").unwrap_or_default();
        let emoji = field("emoji").unwrap_or_default();
        if !game.is_empty() {
            g.playing.push(AroundRoom { room: room.clone(), people: ids(people), game, emoji });
        } else {
            g.in_call.push(AroundRoom { room: room.clone(), people: ids(people), game: String::new(), emoji: String::new() });
        }
    }
    g.call_elsewhere = ids(&elsewhere);
    g.online = ids(&online);
    g.away = if show_away { ids(&away) } else { Vec::new() };
    let mut summary: Vec<String> = Vec::new();
    summary.extend(g.playing.iter().flat_map(|r| r.people.iter().cloned()));
    summary.extend(g.in_call.iter().flat_map(|r| r.people.iter().cloned()));
    summary.extend(g.call_elsewhere.iter().cloned());
    summary.extend(g.online.iter().cloned());
    summary.extend(g.away.iter().cloned());
    g.total = summary.len();
    g.summary = summary;
    g
}

impl AroundGroups {
    pub fn to_json(&self) -> Value {
        let rooms = |l: &[AroundRoom]| -> Vec<Value> {
            l.iter().map(|r| json!({ "room": r.room, "people": r.people, "game": r.game, "emoji": r.emoji })).collect()
        };
        json!({
            "playing": rooms(&self.playing),
            "inCall": rooms(&self.in_call),
            "callElsewhere": self.call_elsewhere,
            "online": self.online,
            "away": self.away,
            "total": self.total,
            "summary": self.summary,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_rules() {
        assert_eq!(ascii_lower("ÄnA Bo"), "Äna bo");
        assert_eq!(ascii_trim("\t Valheim \n"), "Valheim");
        assert_eq!(ascii_trim("\u{a0}x\u{a0}"), "\u{a0}x\u{a0}"); // NBSP is not ASCII whitespace
        assert_eq!(cmp_people("@b:x", "ana", "@a:x", "Ana"), Ordering::Greater); // "ana" == "ana", then "ana" > "Ana"
    }

    #[test]
    fn rate_pair_units() {
        assert_eq!(fmt_rate_pair(2400.0, 1100.0), "↓ 2.4 ↑ 1.1 Mb/s");
        assert_eq!(fmt_rate_pair(395.0, 1200.0), "↓ 395 kb/s ↑ 1.2 Mb/s");
    }

    /// The web's callstats/order.test.ts, case for case: most calls first; ties by name (ASCII case folded, then
    /// exact, then id); time plays no part (2026-09-27).
    #[test]
    fn friends_by_calls_order() {
        let it = |u: &str, calls: i64, name: &str| (u.to_string(), calls, name.to_string());
        let items = vec![
            it("@e:x", 1, "zed"),
            it("@a:x", 3, "bo"),
            it("@d:x", 3, "Émile"),
            it("@b:x", 3, "Ana"),
            it("@c:x", 3, "ana"),
            it("@f:x", 3, "ana"),
        ];
        assert_eq!(friends_by_calls(&items), vec!["@b:x", "@c:x", "@f:x", "@a:x", "@d:x", "@e:x"]);
        assert!(friends_by_calls(&[]).is_empty());
    }
}
