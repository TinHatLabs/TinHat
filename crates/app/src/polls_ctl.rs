//! PollsController + the poll card's JSON — widgets W3 (2026-09-28): Polls (contract WIDGETS.md §17; the web's
//! `features/widgets/polls/{store,PollCard,Polls}.tsx`).
//!
//! * **The card** (`card_json`): what ChatBubble → PollCard.qml paints for a poll start — the shared reader's poll, the
//!   shared tally (the latest valid response per person on or before the first permitted end), the disclosure rule
//!   (`show`), the texts (`state`, `status`, each answer's `votes`), each answer's share in per mille, whether it is mine
//!   / a winner, and `next` = what a click on it sends (the shared `next_choice`; null = nothing). RoomController builds
//!   it in the timeline loader (every start in the window, its responses and ends from the same window) and again after
//!   my own vote / end (the local echo: the card changes at once).
//! * **PollsController** (QML): the widget's Small line (`summary`) and its sample rows (`sampleRows(kind)` — the shared
//!   samples through the shared tally, the web's `sampleRows`), for the gallery and the gate states.

use serde_json::{json, Value};
use tinhat_widgets::polls::{
    next_choice, polls_summary, read_start, row_line, sample_events, share_pm, show_results, state_line, status_line, tally, votes_text, Poll,
    Tally, POLL_CHAT_SAMPLES, POLL_WIDGET_SAMPLES,
};

/// The card for QML: {question, kind, multi, max, answers: [{id, text, n, votes, pm, mine, winner, next}], votes, ended, show,
/// status, state, line (the widget row's), canEnd, pending}.
pub fn card_json(poll: &Poll, t: &Tally, can_end: bool, pending: bool) -> Value {
    let show = show_results(poll, t);
    let answers: Vec<Value> = poll
        .answers
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let n = t.counts.get(i).map_or(0, |c| c.1);
            json!({
                "id": a.id, "text": a.text, "n": n, "votes": votes_text(n),
                "pm": if show { share_pm(n as f64, t.votes as f64) } else { 0.0 },
                "mine": t.mine.contains(&a.id),
                "winner": t.ended && t.winners.contains(&a.id),
                "next": next_choice(poll, &t.mine, &a.id).map_or(Value::Null, |x| json!(x)),
            })
        })
        .collect();
    json!({
        "question": poll.question,
        "kind": if poll.disclosed { "disclosed" } else { "undisclosed" },
        "multi": poll.max > 1,
        "max": poll.max,
        "answers": answers,
        "votes": t.votes,
        "ended": t.ended,
        "show": show,
        "status": status_line(poll, t),
        "state": state_line(poll, t),
        "line": row_line(poll, t),
        "canEnd": can_end && !t.ended && !pending,
        "pending": pending,
    })
}

/// A start event's card from its window: `rels` = the window's responses and ends ({id, type, sender, ts, content}),
/// `enders` = who besides the creator may end it (the redact power). None: not a poll this app shows.
pub fn card_of(ty: &str, content: &Value, start: &str, creator: &str, rels: &[Value], enders: &[String], me: &str, can_redact: bool, pending: bool) -> Option<(Value, Poll, Tally)> {
    let poll = read_start(ty, content)?;
    let t = tally(&poll, start, creator, rels, enders, me);
    let card = card_json(&poll, &t, creator == me || can_redact, pending);
    Some((card, poll, t))
}

/// The widget's rows of the open polls among `starts` ({id, ts, question, line}), newest first.
pub fn open_rows(starts: &[(String, f64, Poll, Tally)]) -> Value {
    let mut open: Vec<&(String, f64, Poll, Tally)> = starts.iter().filter(|(_, _, _, t)| !t.ended).collect();
    open.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Value::Array(open.iter().map(|(id, ts, p, t)| json!({ "id": id, "ts": ts, "question": p.question, "line": row_line(p, t) })).collect())
}

fn day_start_ms() -> f64 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    let off = crate::room::local_offset_ms(now);
    let day = 86_400_000i64;
    ((now + off).div_euclid(day) * day - off) as f64
}

/// The widget's sample rows (the web's `sampleRows` / `sample(kind)`): data = the chat samples + the first two widget
/// samples, many = all of them, else none; for the viewer `me`.
pub fn sample_rows(kind: &str, me: &str) -> Value {
    let list: Vec<&tinhat_widgets::polls::PollSample> = match kind {
        "data" | "preview" => POLL_CHAT_SAMPLES.iter().chain(POLL_WIDGET_SAMPLES.iter().take(2)).collect(),
        "many" => POLL_CHAT_SAMPLES.iter().chain(POLL_WIDGET_SAMPLES.iter()).collect(),
        _ => Vec::new(),
    };
    let day0 = day_start_ms();
    let mut starts: Vec<(String, f64, Poll, Tally)> = Vec::new();
    for s in list {
        let ev = sample_events(s, me, day0);
        let Some(first) = ev.first() else { continue };
        let Some(poll) = read_start(first["type"].as_str().unwrap_or(""), &first["content"]) else { continue };
        let t = tally(&poll, s.id, first["sender"].as_str().unwrap_or(""), &ev, &[], me);
        starts.push((s.id.to_string(), first["ts"].as_f64().unwrap_or(0.0), poll, t));
    }
    open_rows(&starts)
}

#[cxx_qt::bridge]
pub mod qobject {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        type PollsController = super::PollsControllerRust;

        /// QML: summary(open, latest) — the Small line (the shared pollsSummary).
        #[qinvokable]
        fn summary(self: &PollsController, open: i32, latest: &QString) -> QString;
        /// QML: sampleRows(kind) — the sample widget rows [{id, ts, question, line}] (data / many; else []).
        #[qinvokable]
        fn sample_rows(self: &PollsController, kind: &QString) -> QString;
    }
}

use cxx_qt_lib::QString;

#[derive(Default)]
pub struct PollsControllerRust;

impl qobject::PollsController {
    pub fn summary(&self, open: i32, latest: &QString) -> QString {
        QString::from(&polls_summary(open as f64, &latest.to_string()))
    }

    pub fn sample_rows(&self, kind: &QString) -> QString {
        QString::from(&sample_rows(&kind.to_string(), "@you:sample.invalid").to_string())
    }
}
