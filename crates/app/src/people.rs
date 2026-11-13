//! people.rs — the add-people picker's shared rules (2026-11-12, feat/people-search): one picker for room Invite, New DM
//! and space Invite. Pure (dm_target only). Tin Hat Web's `src/features/people/people.ts` applies the same rules; both
//! replay `testvectors/people-search.json` byte for byte (copy in `src/testvectors/people-search.json`).
//!
//!   people_query(text, own)    what the field means: empty / a name / a plain username (YOUR server, the dm_target
//!                              rule) / an address with a server ("@alex:elsewhere.example" or "alex:elsewhere.example")
//!   match_score(name, id, t)   4 exact, 3 prefix, 2 a word's prefix, 1 anywhere, 0 none (display name and username)
//!   person_label(id, name)     "Alex" for someone on your server, "Alex · elsewhere.example" for anyone else (display names
//!                              are not unique: the server is the impostor guard, as on invite cards)
//!   rank_people(…)             the rows: People you know (DM partners first, then by match, then shared rooms; 8),
//!                              On <your server> (the user directory in its own order; 10), and one look-up row for a
//!                              typed username/address nobody listed; members dimmed, never yourself; Enter's pick
//!                              (the typed person first) and its fallback when that look-up fails (yours only)
//!   lookup_outcome(…)          why a profile look-up failed, in plain words; only a refusal still allows an invite

use crate::dm_target::{dm_target, server_of};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum QueryKind {
    Empty,
    Name,
    Username,
    Address,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PeopleQuery {
    pub kind: QueryKind,
    /// the text to match and to search the directory with ("@" and a matrix.to prefix dropped)
    pub term: String,
    /// the user the text names, if it can be one (a plain username → your server)
    pub target: Option<String>,
}

/// The spaces both apps agree on: ASCII only (JavaScript's \s and trim() know U+00A0, U+0085 … and Rust's
/// trim()/split_whitespace() know others), and names compare by code point (`str` order = UTF-8 bytes = code points).
fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0C' | '\r')
}
pub fn atrim(s: &str) -> &str {
    s.trim_matches(is_ws)
}

pub fn people_query(input: &str, own_server: &str) -> PeopleQuery {
    let mut s = atrim(input);
    if s.is_empty() {
        return PeopleQuery { kind: QueryKind::Empty, term: String::new(), target: None };
    }
    let lower = s.to_ascii_lowercase();
    for pre in ["https://matrix.to/#/", "http://matrix.to/#/"] {
        if lower.starts_with(pre) {
            let rest = &s[pre.len()..];
            s = atrim(&rest[..rest.find('?').unwrap_or(rest.len())]);
            break;
        }
    }
    let term = s.strip_prefix('@').unwrap_or(s).to_string();
    match dm_target(input, own_server) {
        None => PeopleQuery { kind: QueryKind::Name, term, target: None },
        Some(t) => PeopleQuery { kind: if term.contains(':') { QueryKind::Address } else { QueryKind::Username }, term, target: Some(t) },
    }
}

/// A typed person → their user id for the signed-in account (a plain username = your own server; dm_target.rs).
pub fn own_target(typed: &str) -> Option<String> {
    let me = crate::session::client().and_then(|c| c.user_id().map(|u| u.to_string())).unwrap_or_default();
    exact_or_typed(typed, &me)
}

/// A full user id is kept exactly as given (a picked row's id; historical ids may have capitals); only typed text is
/// read by the dm_target rule. The web's personTarget.
pub fn exact_or_typed(typed: &str, me: &str) -> Option<String> {
    let t = typed.trim();
    if t.starts_with('@') && matrix_sdk::ruma::UserId::parse(t).is_ok() {
        return Some(t.to_string());
    }
    dm_target(typed, server_of(me))
}

fn local_of(mxid: &str) -> &str {
    let s = mxid.strip_prefix('@').unwrap_or(mxid);
    s.split_once(':').map(|(l, _)| l).unwrap_or(s)
}

/// How well a person matches what was typed: 4 exact, 3 prefix, 2 a word of the name starts with it, 1 anywhere, 0 no.
pub fn match_score(name: &str, mxid: &str, term: &str) -> u8 {
    let t = atrim(term).to_lowercase();
    let t = t.strip_prefix('@').unwrap_or(&t);
    if t.is_empty() {
        return 0;
    }
    let id = mxid.strip_prefix('@').unwrap_or(mxid).to_lowercase();
    if t.contains(':') {
        return if id == t {
            4
        } else if id.starts_with(t) {
            3
        } else {
            0
        };
    }
    let local = local_of(&id);
    let n = atrim(name).to_lowercase();
    if local == t || n == t {
        4
    } else if n.starts_with(t) || local.starts_with(t) {
        3
    } else if n.split(is_ws).any(|w| !w.is_empty() && w.starts_with(t)) {
        2
    } else if n.contains(t) || local.contains(t) {
        1
    } else {
        0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Label {
    /// their server when it isn't yours ("" = yours)
    pub server: String,
    /// the name ("" → the username), plus " · server" for outsiders
    pub line: String,
}

pub fn person_label(mxid: &str, name: &str, own_server: &str) -> Label {
    let shown = match atrim(name) {
        "" => local_of(mxid).to_string(),
        n => n.to_string(),
    };
    let theirs = server_of(mxid);
    let server = if theirs.to_lowercase() == atrim(own_server).to_lowercase() { String::new() } else { theirs.to_string() };
    let line = if server.is_empty() { shown } else { format!("{shown} · {server}") };
    Label { server, line }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Section {
    Known,
    Server,
    Other,
}

pub fn section_title(section: Section, own_server: &str) -> String {
    match section {
        Section::Known => "People you know".to_string(),
        Section::Server => format!("On {own_server}"),
        Section::Other => "On another server".to_string(),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KnownPerson {
    pub mxid: String,
    pub name: String,
    /// an mxc (resolved for display by the controller)
    pub avatar: String,
    /// you have a DM with them
    pub dm: bool,
    /// how many rooms you share (DMs included)
    pub shared: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirectoryHit {
    pub mxid: String,
    pub name: String,
    pub avatar: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PersonRow {
    pub section: Section,
    pub mxid: String,
    pub name: String,
    pub avatar: String,
    pub server: String,
    pub line: String,
    /// already in the room/space: dimmed, can't be picked
    pub member: bool,
    /// nobody listed them: the picker looks the address up before it can be picked
    pub lookup: bool,
    pub dm: bool,
}

pub const KNOWN_MAX: usize = 8;
pub const DIRECTORY_MAX: usize = 10;

pub struct Ranked {
    pub query: PeopleQuery,
    pub rows: Vec<PersonRow>,
    /// Enter's row (-1 = nothing to pick): the person the text names when it names one (a look-up row waits for its
    /// look-up; a look-alike never takes Enter from a typed username), else the first listed person
    pub pick: i32,
    /// Enter once the typed username's look-up has failed: the first listed person on YOUR server (never an outsider)
    pub fallback: i32,
}

pub fn rank_people(input: &str, own: &str, me: &str, known: &[KnownPerson], directory: &[DirectoryHit], members: &[String]) -> Ranked {
    let query = people_query(input, own);
    if query.kind == QueryKind::Empty {
        return Ranked { query, rows: Vec::new(), pick: -1, fallback: -1 };
    }
    let members: std::collections::HashSet<&str> = members.iter().map(String::as_str).collect();
    let mut listed: std::collections::HashSet<String> = std::collections::HashSet::from([me.to_string()]);
    let row = |section: Section, mxid: &str, name: &str, avatar: &str, dm: bool, lookup: bool| {
        let l = person_label(mxid, name, own);
        let shown = match atrim(name) {
            "" => local_of(mxid).to_string(),
            n => n.to_string(),
        };
        PersonRow {
            section,
            mxid: mxid.to_string(),
            name: shown,
            avatar: avatar.to_string(),
            server: l.server,
            line: l.line,
            member: members.contains(mxid),
            lookup,
            dm,
        }
    };
    let mut scored: Vec<(&KnownPerson, u8)> = known
        .iter()
        .filter(|p| p.mxid != me)
        .map(|p| (p, match_score(&p.name, &p.mxid, &query.term)))
        .filter(|(_, sc)| *sc > 0)
        .collect();
    scored.sort_by(|(a, sa), (b, sb)| {
        b.dm.cmp(&a.dm)
            .then(sb.cmp(sa))
            .then(b.shared.cmp(&a.shared))
            .then_with(|| atrim(&a.name).to_lowercase().cmp(&atrim(&b.name).to_lowercase()))
            .then_with(|| a.mxid.cmp(&b.mxid))
    });
    let mut rows = Vec::new();
    for (p, _) in scored.into_iter().take(KNOWN_MAX) {
        rows.push(row(Section::Known, &p.mxid, &p.name, &p.avatar, p.dm, false));
        listed.insert(p.mxid.clone());
    }
    let mut n = 0;
    for h in directory {
        if n >= DIRECTORY_MAX {
            break;
        }
        if listed.contains(&h.mxid) {
            continue;
        }
        rows.push(row(Section::Server, &h.mxid, &h.name, &h.avatar, false, false));
        listed.insert(h.mxid.clone());
        n += 1;
    }
    if let Some(t) = &query.target {
        if !listed.contains(t) {
            let outsider = !person_label(t, "", own).server.is_empty();
            rows.push(row(if outsider { Section::Other } else { Section::Server }, t, "", "", false, true));
        }
    }
    let find = |f: &dyn Fn(&PersonRow) -> bool| rows.iter().position(f).map(|i| i as i32);
    // when the text names someone, Enter never lands on another server's person instead (they may be a member already, or you)
    let named = query.target.is_some();
    // A full address ("@alice:their.server") means exactly that person: Enter never lands on anyone else, not when they
    // are a member already and not after their look-up failed (a local person's display name may copy the address).
    let exact = query.kind == QueryKind::Address;
    let pick = find(&|r| Some(&r.mxid) == query.target.as_ref() && !r.member)
        .or_else(|| if exact { None } else { find(&|r| !r.member && !r.lookup && (!named || r.server.is_empty())) })
        .unwrap_or(-1);
    let fallback = if exact { -1 } else { find(&|r| !r.member && !r.lookup && r.server.is_empty()).unwrap_or(-1) };
    Ranked { query, rows, pick, fallback }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LookupKind {
    Found,
    NotFound,
    NoFederation,
    Refused,
    Unreachable,
    RateLimited,
}

impl LookupKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LookupKind::Found => "found",
            LookupKind::NotFound => "notfound",
            LookupKind::NoFederation => "nofederation",
            LookupKind::Refused => "refused",
            LookupKind::Unreachable => "unreachable",
            LookupKind::RateLimited => "ratelimited",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub kind: LookupKind,
    pub pickable: bool,
    pub text: String,
}

/// A profile look-up's answer → what the picker says. `status` 0 = no answer at all (network).
pub fn lookup_outcome(status: u16, errcode: &str, message: &str, target: &str, own_server: &str) -> Outcome {
    let server = person_label(target, "", own_server).server;
    let local = local_of(target);
    let o = |kind, pickable, text: String| Outcome { kind, pickable, text };
    if (200..300).contains(&status) {
        return o(LookupKind::Found, true, String::new());
    }
    if status == 429 || errcode == "M_LIMIT_EXCEEDED" {
        return o(LookupKind::RateLimited, false, "Too many look-ups at once. Try again in a moment.".to_string());
    }
    if status == 404 || errcode == "M_NOT_FOUND" {
        let text = if server.is_empty() {
            format!("There's no one called {local} on your server. Check the spelling.")
        } else {
            format!("{server} has no one called {local}. Check the spelling.")
        };
        return o(LookupKind::NotFound, false, text);
    }
    if status == 403 && !server.is_empty() && message.to_lowercase().contains("federation denied") {
        return o(
            LookupKind::NoFederation,
            false,
            format!("Your server doesn't connect to {server}, so people there can't be reached from here. The owner of your server can change that."),
        );
    }
    if status == 401 || status == 403 {
        let who = if server.is_empty() { "Your server" } else { server.as_str() };
        return o(LookupKind::Refused, true, format!("{who} only shows profiles to people who already share a room. You can still invite them."));
    }
    let text = if server.is_empty() {
        "Couldn't reach your server right now. Try again in a moment.".to_string()
    } else {
        format!("Couldn't reach {server} right now. Their server may be down; try again later.")
    };
    o(LookupKind::Unreachable, false, text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn vectors() -> Value {
        serde_json::from_str(include_str!("testvectors/people-search.json")).unwrap()
    }
    fn s(v: &Value) -> &str {
        v.as_str().unwrap_or("")
    }

    #[test]
    fn query_vectors() {
        let v = vectors();
        let cases = v["query"].as_array().unwrap();
        assert!(cases.len() >= 15);
        for c in cases {
            let q = people_query(s(&c["input"]), s(&c["server"]));
            assert_eq!(serde_json::to_value(&q).unwrap(), c["want"], "{c}");
        }
    }

    #[test]
    fn label_vectors() {
        for c in vectors()["label"].as_array().unwrap() {
            let l = person_label(s(&c["mxid"]), s(&c["name"]), s(&c["server"]));
            assert_eq!(serde_json::to_value(&l).unwrap(), c["want"], "{c}");
        }
    }

    #[test]
    fn score_vectors() {
        for c in vectors()["score"].as_array().unwrap() {
            assert_eq!(match_score(s(&c["name"]), s(&c["mxid"]), s(&c["term"])) as u64, c["want"].as_u64().unwrap(), "{c}");
        }
    }

    #[test]
    fn rank_vectors() {
        let v = vectors();
        let cases = v["rank"].as_array().unwrap();
        assert!(cases.len() >= 13);
        for c in cases {
            let known: Vec<KnownPerson> = c["known"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| KnownPerson {
                    mxid: s(&p["mxid"]).into(),
                    name: s(&p["name"]).into(),
                    avatar: s(&p["avatar"]).into(),
                    dm: p["dm"].as_bool().unwrap(),
                    shared: p["shared"].as_u64().unwrap() as u32,
                })
                .collect();
            let dir: Vec<DirectoryHit> = c["directory"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| DirectoryHit { mxid: s(&p["mxid"]).into(), name: s(&p["name"]).into(), avatar: s(&p["avatar"]).into() })
                .collect();
            let members: Vec<String> = c["members"].as_array().unwrap().iter().map(|m| s(m).to_string()).collect();
            let got = rank_people(s(&c["input"]), s(&c["own"]), s(&c["me"]), &known, &dir, &members);
            let rows: Vec<Value> = got
                .rows
                .iter()
                .map(|r| json!({"section": r.section, "mxid": r.mxid, "server": r.server, "member": r.member, "lookup": r.lookup}))
                .collect();
            assert_eq!(json!({"rows": rows, "pick": got.pick, "fallback": got.fallback}), c["want"], "{}", c["what"]);
        }
    }

    #[test]
    fn rank_rows_keep_name_and_picture() {
        let known = [KnownPerson { mxid: "@al:home.example".into(), name: "Al".into(), avatar: "mxc://x/y".into(), dm: true, shared: 1 }];
        let dir = [DirectoryHit { mxid: "@alf:far.example".into(), name: String::new(), avatar: String::new() }];
        let got = rank_people("al", "home.example", "@me:home.example", &known, &dir, &[]);
        assert_eq!((got.rows[0].name.as_str(), got.rows[0].avatar.as_str(), got.rows[0].line.as_str(), got.rows[0].dm), ("Al", "mxc://x/y", "Al", true));
        assert_eq!((got.rows[1].name.as_str(), got.rows[1].line.as_str()), ("alf", "alf · far.example"));
    }

    #[test]
    fn lookup_vectors() {
        let v = vectors();
        let cases = v["lookup"].as_array().unwrap();
        assert!(cases.len() >= 12);
        for c in cases {
            let o = lookup_outcome(c["status"].as_u64().unwrap() as u16, s(&c["errcode"]), s(&c["message"]), s(&c["target"]), s(&c["own"]));
            assert_eq!(json!({"kind": o.kind.as_str(), "pickable": o.pickable, "text": o.text}), c["want"], "{c}");
        }
    }

    #[test]
    fn section_titles() {
        for c in vectors()["sections"].as_array().unwrap() {
            let sec = match s(&c["section"]) {
                "known" => Section::Known,
                "server" => Section::Server,
                _ => Section::Other,
            };
            assert_eq!(section_title(sec, s(&c["own"])), s(&c["want"]));
        }
    }

    /// The words (2026-11-12): there are no friends or contacts in Tin Hat, only people.
    #[test]
    fn never_friends_or_contacts() {
        let code: String = [include_str!("people.rs"), include_str!("people_ctl.rs"), include_str!("../qml/PeoplePicker.qml")]
            .iter()
            .flat_map(|f| f.lines())
            .filter(|l| !l.trim_start().starts_with("//") && !l.contains("never_") && !l.contains("concat!"))
            .collect::<Vec<_>>()
            .join("\n")
            .to_lowercase();
        for w in [concat!("fri", "end"), concat!("con", "tact")] {
            assert!(!code.contains(w), "{w}");
        }
    }

    /// The byte copy equals Tin Hat Web's when a web checkout is named by `TINHAT_WEB_DIR` (skipped otherwise).
    #[test]
    fn vectors_are_the_webs() {
        let Some(dir) = std::env::var_os("TINHAT_WEB_DIR") else { return };
        // named → the copy must exist (a missing file fails, it isn't skipped)
        let path = std::path::Path::new(&dir).join("src/features/people/testvectors/people-search.json");
        let theirs = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(include_str!("testvectors/people-search.json"), theirs);
    }
}
