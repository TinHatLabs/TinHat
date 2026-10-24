//! polls — the Polls widget's and the poll card's shared logic (contract WIDGETS.md §17, vectors
//! testvectors/widgets/polls/*). A line-for-line port of the web's `src/widgets/polls.ts` (widgets W3, 2026-09-28):
//! MSC3381 polls as Element sends and reads them — `m.poll.start` / `m.poll.response` / `m.poll.end` and their
//! `org.matrix.msc3381.*` names — read from every shape in use, written in the shape Element sends; the tally (the latest
//! valid response per person on or before the first permitted end), the disclosure rule, the lines the card and the
//! widget show, and the end message. Strings are compared and cut in code points (Rust's `str` order), whitespace is
//! ASCII only, numbers are doubles.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Map, Value};

/// A poll offers at most this many answers (the first 20 are read; the dialog offers 20).
pub const ANSWERS_MAX: usize = 20;
/// The New poll dialog's limits (code points): the question and each answer.
pub const QUESTION_MAX: usize = 340;
pub const ANSWER_MAX: usize = 340;

pub const START_TYPES: [&str; 2] = ["m.poll.start", "org.matrix.msc3381.poll.start"];
pub const RESPONSE_TYPES: [&str; 2] = ["m.poll.response", "org.matrix.msc3381.poll.response"];
pub const END_TYPES: [&str; 2] = ["m.poll.end", "org.matrix.msc3381.poll.end"];
/// What we send (Element's names: it reads these, and the stable names too).
pub const START_TYPE: &str = "org.matrix.msc3381.poll.start";
pub const RESPONSE_TYPE: &str = "org.matrix.msc3381.poll.response";
pub const END_TYPE: &str = "org.matrix.msc3381.poll.end";
const TEXT: &str = "org.matrix.msc1767.text";
const DISCLOSED: [&str; 3] = ["m.disclosed", "m.poll.disclosed", "org.matrix.msc3381.poll.disclosed"];
const REFERENCE: [&str; 2] = ["m.reference", "org.matrix.msc3267.reference"];

#[derive(Clone, Debug, PartialEq)]
pub struct PollAnswer {
    pub id: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Poll {
    pub question: String,
    /// true = disclosed (results while open, once you voted), false = undisclosed
    pub disclosed: bool,
    /// how many answers one person may pick (1 = a single choice)
    pub max: usize,
    pub answers: Vec<PollAnswer>,
}

impl Poll {
    pub fn to_json(&self) -> Value {
        json!({
            "question": self.question,
            "kind": if self.disclosed { "disclosed" } else { "undisclosed" },
            "max": self.max,
            "answers": self.answers.iter().map(|a| json!({ "id": a.id, "text": a.text })).collect::<Vec<_>>(),
        })
    }
}

fn is_ws(c: char) -> bool {
    c == ' ' || ('\t'..='\r').contains(&c)
}

/// Trim ASCII whitespace (U+0009–U+000D, U+0020) at both ends.
pub fn trim_ascii(s: &str) -> &str {
    s.trim_matches(is_ws)
}

fn plain_of(list: Option<&Value>) -> Option<String> {
    for r in list?.as_array()? {
        let Some(o) = r.as_object() else { continue };
        let Some(body) = o.get("body").and_then(Value::as_str) else { continue };
        match o.get("mimetype") {
            None => return Some(body.to_string()),
            Some(Value::String(m)) if m == "text/plain" => return Some(body.to_string()),
            _ => {}
        }
    }
    None
}

/// The plain text of a content block (see the web's `textOf`): `m.text` (a string, or the spec's array — the first
/// plain item), `org.matrix.msc1767.text` (a string), `m.message` / `org.matrix.msc1767.message` (arrays), `body`.
/// ASCII-trimmed; "" or none → None.
pub fn text_of(block: &Value) -> Option<String> {
    let b = block.as_object()?;
    let pick = || -> Option<String> {
        match b.get("m.text") {
            Some(Value::String(s)) => return Some(s.clone()),
            v => {
                if let Some(t) = plain_of(v) {
                    return Some(t);
                }
            }
        }
        if let Some(Value::String(s)) = b.get(TEXT) {
            return Some(s.clone());
        }
        if let Some(t) = plain_of(b.get("m.message")).or_else(|| plain_of(b.get("org.matrix.msc1767.message"))) {
            return Some(t);
        }
        b.get("body").and_then(Value::as_str).map(String::from)
    };
    let t = pick()?;
    let s = trim_ascii(&t);
    (!s.is_empty()).then(|| s.to_string())
}

/// A poll start → the poll, or None (see the web's `readStart` for every rule).
pub fn read_start(ty: &str, content: &Value) -> Option<Poll> {
    if !START_TYPES.contains(&ty) {
        return None;
    }
    let c = content.as_object()?;
    let block = ["m.poll", "m.poll.start", "org.matrix.msc3381.poll.start"].iter().find_map(|k| c.get(*k).and_then(Value::as_object))?;
    let question = text_of(block.get("question").unwrap_or(&Value::Null))?;
    let disclosed = block.get("kind").and_then(Value::as_str).is_some_and(|k| DISCLOSED.contains(&k));
    let list = block.get("answers")?.as_array()?;
    let mut answers = Vec::new();
    let mut ids = HashSet::new();
    for a in list.iter().take(ANSWERS_MAX) {
        let o = a.as_object()?;
        let id = match o.get("m.id") {
            Some(Value::String(s)) => s.clone(),
            _ => o.get("id")?.as_str()?.to_string(),
        };
        let n = id.chars().count();
        if !(1..=100).contains(&n) || ids.contains(&id) {
            return None;
        }
        let text = text_of(a)?;
        ids.insert(id.clone());
        answers.push(PollAnswer { id, text });
    }
    if answers.is_empty() {
        return None;
    }
    let max = match block.get("max_selections").and_then(Value::as_f64) {
        Some(m) if m.is_finite() && m >= 1.0 => (m.floor() as usize).min(answers.len()),
        _ => 1,
    };
    Some(Poll { question, disclosed, max, answers })
}

fn ref_of(c: &Map<String, Value>) -> Option<String> {
    let rel = c.get("m.relates_to")?.as_object()?;
    let rt = rel.get("rel_type")?.as_str()?;
    if !REFERENCE.contains(&rt) {
        return None;
    }
    let e = rel.get("event_id")?.as_str()?;
    e.starts_with('$').then(|| e.to_string())
}

/// A poll response → (start, answers) — answers None = spoiled (see the web's `readResponse`).
pub fn read_response(ty: &str, content: &Value) -> Option<(String, Option<Vec<String>>)> {
    if !RESPONSE_TYPES.contains(&ty) {
        return None;
    }
    let c = content.as_object()?;
    let start = ref_of(c)?;
    let block_of = |k: &str| c.get(k).and_then(Value::as_object).and_then(|b| b.get("answers")).cloned();
    let sel: Option<Value> = if c.contains_key("m.selections") {
        c.get("m.selections").cloned()
    } else if c.get("m.poll.response").is_some_and(Value::is_object) {
        block_of("m.poll.response")
    } else {
        block_of("org.matrix.msc3381.poll.response")
    };
    let answers = sel
        .as_ref()
        .and_then(Value::as_array)
        .and_then(|a| a.iter().map(|x| x.as_str().map(String::from)).collect::<Option<Vec<String>>>());
    Some((start, answers))
}

/// A poll end → the start it ends (see the web's `readEnd`).
pub fn read_end(ty: &str, content: &Value) -> Option<String> {
    if !END_TYPES.contains(&ty) {
        return None;
    }
    ref_of(content.as_object()?)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tally {
    pub ended: bool,
    pub end_ts: Option<f64>,
    /// every answer's count, in answer order
    pub counts: Vec<(String, u32)>,
    pub votes: u32,
    pub mine: Vec<String>,
    pub winners: Vec<String>,
}

impl Tally {
    pub fn to_json(&self) -> Value {
        json!({
            "ended": self.ended,
            "endTs": self.end_ts.map_or(Value::Null, |t| json!(t)),
            "counts": self.counts.iter().map(|(id, n)| json!({ "id": id, "n": n })).collect::<Vec<_>>(),
            "votes": self.votes,
            "mine": self.mine,
            "winners": self.winners,
        })
    }
}

fn choice_of(poll: &Poll, answers: &Option<Vec<String>>, valid: &HashSet<&str>) -> Option<Vec<String>> {
    let answers = answers.as_ref()?;
    if answers.is_empty() {
        return None;
    }
    let mut out: Vec<String> = Vec::new();
    for a in answers {
        if !valid.contains(a.as_str()) {
            return None;
        }
        if !out.contains(a) {
            out.push(a.clone());
        }
    }
    out.truncate(poll.max);
    Some(out)
}

/// The tally (see the web's `tally` for every rule). `events` = any events as `{id, type, sender, ts, content}`.
pub fn tally(poll: &Poll, start: &str, creator: &str, events: &[Value], enders: &[String], me: &str) -> Tally {
    let mut seen: HashSet<String> = HashSet::new();
    let mut end: Option<(f64, String)> = None;
    struct Resp {
        sender: String,
        ts: f64,
        id: String,
        answers: Option<Vec<String>>,
    }
    let mut rs: Vec<Resp> = Vec::new();
    for e in events {
        let Some(o) = e.as_object() else { continue };
        let (Some(id), Some(sender), Some(ts)) = (o.get("id").and_then(Value::as_str), o.get("sender").and_then(Value::as_str), o.get("ts").and_then(Value::as_f64)) else {
            continue;
        };
        if id.is_empty() || seen.contains(id) || !ts.is_finite() {
            continue;
        }
        seen.insert(id.to_string());
        let ty = o.get("type").and_then(Value::as_str).unwrap_or("");
        let content = o.get("content").unwrap_or(&Value::Null);
        if let Some(s) = read_end(ty, content) {
            let permitted = sender == creator || enders.iter().any(|x| x == sender);
            let earlier = match &end {
                None => true,
                Some((et, eid)) => ts < *et || (ts == *et && id < eid.as_str()),
            };
            if s == start && permitted && earlier {
                end = Some((ts, id.to_string()));
            }
            continue;
        }
        if let Some((s, answers)) = read_response(ty, content) {
            if s == start {
                rs.push(Resp { sender: sender.to_string(), ts, id: id.to_string(), answers });
            }
        }
    }
    let mut latest: HashMap<String, usize> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for (i, r) in rs.iter().enumerate() {
        if let Some((et, _)) = &end {
            if r.ts > *et {
                continue;
            }
        }
        match latest.get(&r.sender) {
            None => {
                order.push(r.sender.clone());
                latest.insert(r.sender.clone(), i);
            }
            Some(&j) => {
                let cur = &rs[j];
                if r.ts > cur.ts || (r.ts == cur.ts && r.id.as_str() > cur.id.as_str()) {
                    latest.insert(r.sender.clone(), i);
                }
            }
        }
    }
    let valid: HashSet<&str> = poll.answers.iter().map(|a| a.id.as_str()).collect();
    let mut counts: HashMap<&str, u32> = poll.answers.iter().map(|a| (a.id.as_str(), 0)).collect();
    let mut votes = 0;
    let mut mine: Vec<String> = Vec::new();
    for sender in &order {
        let r = &rs[latest[sender]];
        let Some(pick) = choice_of(poll, &r.answers, &valid) else { continue };
        votes += 1;
        for a in &pick {
            if let Some(n) = counts.get_mut(a.as_str()) {
                *n += 1;
            }
        }
        if sender == me {
            mine = pick;
        }
    }
    let list: Vec<(String, u32)> = poll.answers.iter().map(|a| (a.id.clone(), counts[a.id.as_str()])).collect();
    let top = list.iter().map(|(_, n)| *n).max().unwrap_or(0);
    let winners = if top > 0 { list.iter().filter(|(_, n)| *n == top).map(|(id, _)| id.clone()).collect() } else { Vec::new() };
    Tally { ended: end.is_some(), end_ts: end.map(|(t, _)| t), counts: list, votes, mine, winners }
}

/// The disclosure rule: results show once the poll has ended, or — disclosed polls — once I voted.
pub fn show_results(poll: &Poll, t: &Tally) -> bool {
    t.ended || (poll.disclosed && !t.mine.is_empty())
}

/// "1 vote" / "<n> votes" (see the web's `votesText`).
pub fn votes_text(n: u32) -> String {
    if n == 1 {
        "1 vote".into()
    } else {
        format!("{n} votes")
    }
}

/// The card's status line (see the web's `statusLine`).
pub fn status_line(poll: &Poll, t: &Tally) -> String {
    if t.ended {
        return if t.votes == 0 { "No votes were cast".into() } else { format!("Final result based on {}", votes_text(t.votes)) };
    }
    if poll.disclosed {
        if !t.mine.is_empty() {
            return format!("Based on {}", votes_text(t.votes));
        }
        return if t.votes == 0 { "No votes yet".into() } else { format!("{} cast · vote to see the results", votes_text(t.votes)) };
    }
    format!("Results will be visible when the poll ends{}", if t.mine.is_empty() { "" } else { " · you voted" })
}

/// An answer's share for its bar, in whole per mille of the votes: r(n × 1000 / votes).
pub fn share_pm(n: f64, votes: f64) -> f64 {
    if !(votes > 0.0) || !(n > 0.0) {
        return 0.0;
    }
    ((n * 1000.0) / votes + 0.5).floor()
}

/// The card's head after "POLL" (see the web's `stateLine`).
pub fn state_line(poll: &Poll, t: &Tally) -> String {
    if t.ended {
        return "Ended".into();
    }
    if !poll.disclosed {
        return "Results hidden until it ends".into();
    }
    if poll.max > 1 {
        format!("Pick up to {}", poll.max)
    } else {
        String::new()
    }
}

/// The widget row's second line (see the web's `rowLine`).
pub fn row_line(poll: &Poll, t: &Tally) -> String {
    let you = if t.mine.is_empty() { "vote" } else { "you voted" };
    if poll.disclosed {
        format!("{} · {you}", votes_text(t.votes))
    } else {
        format!("Results hidden until it ends · {you}")
    }
}

/// The Small / collapsed line (see the web's `pollsSummary`).
pub fn polls_summary(open: f64, latest: &str) -> String {
    if !(open > 0.0) {
        return "No open polls".into();
    }
    if open == 1.0 {
        return if latest.is_empty() { "1 open poll".into() } else { format!("1 open poll · {latest}") };
    }
    let n = tinhat_theme::canon::js_number(open);
    if latest.is_empty() {
        format!("{n} open polls")
    } else {
        format!("{n} open polls · latest: {latest}")
    }
}

/// Names joined for the end message (see the web's `joinNames`).
pub fn join_names(names: &[String]) -> String {
    match names.len() {
        0 => String::new(),
        1 => names[0].clone(),
        2 | 3 => format!("{} and {}", names[..names.len() - 1].join(", "), names[names.len() - 1]),
        n => {
            let rest = n - 3;
            format!("{} and {}", names[..3].join(", "), if rest == 1 { "1 other".to_string() } else { format!("{rest} others") })
        }
    }
}

/// The end message (see the web's `endText`).
pub fn end_text(poll: &Poll, t: &Tally) -> String {
    if t.winners.is_empty() {
        return "The poll has ended. No votes were cast.".into();
    }
    let text = |id: &String| poll.answers.iter().find(|a| &a.id == id).map(|a| a.text.clone()).unwrap_or_default();
    format!("The poll has ended. Top answer: {}", join_names(&t.winners.iter().map(text).collect::<Vec<_>>()))
}

/// The New poll dialog's input → the start event's content, or (code, message) (see the web's `startContent`).
pub fn start_content(question: &str, answers: &[String], disclosed: bool, ids: &[String]) -> Result<Value, (&'static str, &'static str)> {
    let q = trim_ascii(question);
    let qn = q.chars().count();
    if qn < 1 {
        return Err(("E_QUESTION", "Ask a question"));
    }
    if qn > QUESTION_MAX {
        return Err(("E_QUESTION", "The question is too long — 340 characters at most"));
    }
    let as_: Vec<&str> = answers.iter().map(|a| trim_ascii(a)).filter(|a| !a.is_empty()).collect();
    if as_.len() < 2 {
        return Err(("E_ANSWERS", "Add at least 2 answers"));
    }
    if as_.len() > ANSWERS_MAX {
        return Err(("E_ANSWERS", "20 answers at most"));
    }
    if as_.iter().any(|a| a.chars().count() > ANSWER_MAX) {
        return Err(("E_ANSWER_LONG", "An answer is too long — 340 characters at most"));
    }
    if ids.len() < as_.len() {
        return Err(("E_ANSWERS", "Add at least 2 answers"));
    }
    let fallback = format!("{q}\n{}", as_.iter().enumerate().map(|(i, a)| format!("{}. {a}", i + 1)).collect::<Vec<_>>().join("\n"));
    Ok(json!({
        START_TYPE: {
            "question": { TEXT: q, "body": q, "msgtype": "m.text" },
            "kind": if disclosed { "org.matrix.msc3381.poll.disclosed" } else { "org.matrix.msc3381.poll.undisclosed" },
            "max_selections": 1,
            "answers": as_.iter().enumerate().map(|(i, a)| json!({ "id": ids[i], TEXT: a })).collect::<Vec<_>>(),
        },
        TEXT: fallback,
    }))
}

/// A vote: `org.matrix.msc3381.poll.response` {answers} referencing the start.
pub fn response_content(start: &str, answers: &[String]) -> Value {
    json!({ "m.relates_to": { "rel_type": "m.reference", "event_id": start }, RESPONSE_TYPE: { "answers": answers } })
}

/// Ending: `org.matrix.msc3381.poll.end` {} referencing the start + the end message as text.
pub fn end_content(start: &str, text: &str) -> Value {
    json!({ "m.relates_to": { "rel_type": "m.reference", "event_id": start }, END_TYPE: {}, TEXT: text, "body": text, "msgtype": "m.text" })
}

/// What a click on an answer sends (see the web's `nextChoice`); None = nothing to send.
pub fn next_choice(poll: &Poll, mine: &[String], clicked: &str) -> Option<Vec<String>> {
    if !poll.answers.iter().any(|a| a.id == clicked) {
        return None;
    }
    if poll.max <= 1 {
        return if mine.len() == 1 && mine[0] == clicked { None } else { Some(vec![clicked.to_string()]) };
    }
    let sel: Vec<String> = if mine.iter().any(|m| m == clicked) {
        mine.iter().filter(|m| m.as_str() != clicked).cloned().collect()
    } else {
        mine.iter().cloned().chain(std::iter::once(clicked.to_string())).collect()
    };
    if sel.is_empty() || sel.len() > poll.max {
        return None;
    }
    Some(poll.answers.iter().filter(|a| sel.contains(&a.id)).map(|a| a.id.clone()).collect())
}

// ---- the chat samples (the gates' poll cards; both apps build the same events) ------------------------------------------

pub struct PollSample {
    pub id: &'static str,
    /// the creator ("" when `mine`: the viewer)
    pub sender: &'static str,
    pub mine: bool,
    /// today's local time (h, m) of the start
    pub at: (u32, u32),
    pub question: &'static str,
    pub disclosed: bool,
    pub answers: &'static [(&'static str, &'static str)],
    /// (who, answer id) in order, one a minute after the start ("me" = the viewer)
    pub votes: &'static [(&'static str, &'static str)],
    pub ended: bool,
}

/// The web's `POLL_CHAT_SAMPLES` (`TINHAT_CHAT_SAMPLES=polls`; vectors polls/samples.json).
pub const POLL_CHAT_SAMPLES: &[PollSample] = &[
    PollSample {
        id: "$w3-sample-poll-1",
        sender: "@Ana:sample.invalid",
        mine: false,
        at: (20, 30),
        question: "What are we playing tonight?",
        disclosed: true,
        answers: &[("valheim", "Valheim"), ("zomboid", "Project Zomboid"), ("league", "League of Legends"), ("finals", "The Finals")],
        votes: &[("@Ana:sample.invalid", "valheim"), ("@Bo:sample.invalid", "valheim"), ("@Cy:sample.invalid", "zomboid"), ("me", "valheim")],
        ended: false,
    },
    PollSample {
        id: "$w3-sample-poll-2",
        sender: "",
        mine: true,
        at: (20, 36),
        question: "Start time for raid night?",
        disclosed: false,
        answers: &[("8pm", "8 PM"), ("9pm", "9 PM"), ("10pm", "10 PM")],
        votes: &[("@Bo:sample.invalid", "9pm"), ("@Dee:sample.invalid", "8pm")],
        ended: false,
    },
    PollSample {
        id: "$w3-sample-poll-3",
        sender: "@Bo:sample.invalid",
        mine: false,
        at: (20, 42),
        question: "Best boss so far?",
        disclosed: true,
        answers: &[("elder", "The Elder"), ("moder", "Moder"), ("yagluth", "Yagluth")],
        votes: &[("@Ana:sample.invalid", "moder"), ("@Bo:sample.invalid", "yagluth"), ("@Cy:sample.invalid", "moder"), ("me", "elder")],
        ended: true,
    },
];

/// A sample list as the web's JSON (the vectors' `samples` / `widget`).
pub fn samples_json(list: &[PollSample]) -> Value {
    Value::Array(
        list
            .iter()
            .map(|s| {
                json!({
                    "id": s.id, "sender": s.sender, "mine": s.mine, "at": [s.at.0, s.at.1], "question": s.question,
                    "kind": if s.disclosed { "disclosed" } else { "undisclosed" },
                    "answers": s.answers.iter().map(|(i, t)| json!([i, t])).collect::<Vec<_>>(),
                    "votes": s.votes.iter().map(|(w, a)| json!([w, a])).collect::<Vec<_>>(),
                    "ended": s.ended,
                })
            })
            .collect(),
    )
}

/// A sample poll's events (see the web's `sampleEvents`): `{id, type, sender, ts, content}` values.
pub fn sample_events(s: &PollSample, me: &str, day0: f64) -> Vec<Value> {
    let creator = if s.mine { me.to_string() } else { s.sender.to_string() };
    let t0 = day0 + ((s.at.0 * 60 + s.at.1) as f64) * 60000.0;
    let texts: Vec<String> = s.answers.iter().map(|(_, t)| t.to_string()).collect();
    let ids: Vec<String> = s.answers.iter().map(|(i, _)| i.to_string()).collect();
    let content = start_content(s.question, &texts, s.disclosed, &ids).unwrap_or_else(|_| json!({}));
    let mut out = vec![json!({ "id": s.id, "type": START_TYPE, "sender": creator, "ts": t0, "content": content })];
    for (i, (who, a)) in s.votes.iter().enumerate() {
        let sender: &str = if *who == "me" { me } else { who };
        out.push(json!({ "id": format!("{}-r{i}", s.id), "type": RESPONSE_TYPE, "sender": sender,
                         "ts": t0 + ((i + 1) as f64) * 60000.0, "content": response_content(s.id, &[a.to_string()]) }));
    }
    if s.ended {
        let text = read_start(START_TYPE, &out[0]["content"]).map(|p| end_text(&p, &tally(&p, s.id, &creator, &out, &[], me))).unwrap_or_default();
        out.push(json!({ "id": format!("{}-end", s.id), "type": END_TYPE, "sender": creator,
                         "ts": t0 + ((s.votes.len() + 1) as f64) * 60000.0, "content": end_content(s.id, &text) }));
    }
    out
}

/// The web's `POLL_WIDGET_SAMPLES`: more sample polls for the widget's states (never in the chat samples).
pub const POLL_WIDGET_SAMPLES: &[PollSample] = &[
    PollSample {
        id: "$w3-sample-poll-4",
        sender: "@Cy:sample.invalid",
        mine: false,
        at: (19, 50),
        question: "Which biome for Friday?",
        disclosed: true,
        answers: &[("meadows", "Meadows"), ("swamp", "Swamp"), ("mountains", "Mountains")],
        votes: &[("@Cy:sample.invalid", "swamp"), ("@Dee:sample.invalid", "meadows")],
        ended: false,
    },
    PollSample {
        id: "$w3-sample-poll-5",
        sender: "@Dee:sample.invalid",
        mine: false,
        at: (19, 20),
        question: "Pizza or tacos for game night?",
        disclosed: true,
        answers: &[("pizza", "Pizza"), ("tacos", "Tacos")],
        votes: &[("me", "tacos"), ("@Ana:sample.invalid", "pizza"), ("@Eli:sample.invalid", "tacos")],
        ended: false,
    },
    PollSample {
        id: "$w3-sample-poll-6",
        sender: "@Eli:sample.invalid",
        mine: false,
        at: (18, 5),
        question: "Should we restart the Valheim world after the next patch?",
        disclosed: false,
        answers: &[("yes", "Yes, a fresh start"), ("no", "No, keep it")],
        votes: &[("@Fay:sample.invalid", "no")],
        ended: false,
    },
    PollSample {
        id: "$w3-sample-poll-7",
        sender: "",
        mine: true,
        at: (17, 40),
        question: "Keep the voice bitrate at 96 kbps?",
        disclosed: true,
        answers: &[("keep", "Keep it"), ("raise", "Raise it")],
        votes: &[],
        ended: false,
    },
];
