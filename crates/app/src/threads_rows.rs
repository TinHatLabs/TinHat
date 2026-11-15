//! threads (docs/THREADS.md): what threads change in the chat's rows — the mirror of Tin Hat Web's
//! `src/features/chat/threads/rows.ts` (the same functions, the same test cases). room.rs's row builder calls it on the
//! window's raw event JSON:
//!   thread_plan    which thread each event is in, which events are rows in this view (main: thread replies leave; a
//!                  thread's panel: its own events only), and the replies seen per root
//!   root_thread    the summary under a root in the main timeline (server bundle + seen replies, my unread count)
//!   fallback_quote whether a thread message's m.in_reply_to is only the spec's fallback (no quote shown)
//!   merge_thread   a thread's events for its panel: the room window's + the fetched /relations pages + the root
//! The pure rules themselves are tinhat_widgets::threads (the shared vectors).

use serde_json::Value;
use std::collections::HashMap;
use tinhat_widgets::threads::{bundle_of, relation_target, summarize, thread_ids, thread_of, thread_unread, MyReceipt, ReplyRef};

pub(crate) fn ev_id(e: &Value) -> &str {
    e.get("event_id").and_then(|i| i.as_str()).unwrap_or("")
}
pub(crate) fn ev_ts(e: &Value) -> i64 {
    e.get("origin_server_ts").and_then(|t| t.as_i64()).unwrap_or(0)
}
fn ev_sender(e: &Value) -> &str {
    e.get("sender").and_then(|s| s.as_str()).unwrap_or("")
}
fn ev_redacted(e: &Value) -> bool {
    e.get("unsigned").and_then(|u| u.get("redacted_because")).is_some()
}
fn ev_content(e: &Value) -> &Value {
    static NULL: Value = Value::Null;
    e.get("content").unwrap_or(&NULL)
}

pub(crate) struct Plan {
    /// per event: its thread ("" = main)
    pub tids: Vec<String>,
    /// per event: shown in this view
    pub in_view: Vec<bool>,
    /// main view: per root, the replies seen (direct m.thread events)
    pub seen: HashMap<String, Vec<ReplyRef>>,
    /// every event's ts (a receipt reads up to its event's ts when that event is loaded, review #5)
    pub ts_of: HashMap<String, i64>,
}

/// In a thread's view: its own events, the root, and what relates to the root itself (its reactions and edits).
fn in_thread_view(e: &Value, tid: &str, root: &str) -> bool {
    tid == root || ev_id(e) == root || (tid.is_empty() && relation_target(ev_content(e)) == root)
}

/// `view` None = the main timeline, Some(root) = that thread's panel.
pub(crate) fn thread_plan(evs: &[Value], view: Option<&str>) -> Plan {
    let tids = thread_ids(evs);
    let in_view = evs
        .iter()
        .zip(tids.iter())
        .map(|(e, t)| match view {
            None => t.is_empty(),
            Some(root) => in_thread_view(e, t, root),
        })
        .collect();
    let ts_of: HashMap<String, i64> = evs.iter().map(|e| (ev_id(e).to_string(), ev_ts(e))).collect();
    let mut seen: HashMap<String, Vec<ReplyRef>> = HashMap::new();
    if view.is_none() {
        for e in evs {
            let root = thread_of(ev_content(e));
            if root.is_empty() {
                continue;
            }
            seen.entry(root).or_default().push(ReplyRef { id: ev_id(e).to_string(), sender: ev_sender(e).to_string(), ts: ev_ts(e), redacted: ev_redacted(e) });
        }
    }
    Plan { tids, in_view, seen, ts_of }
}

/// The summary under a root, before names (room.rs resolves them); None = no thread on it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RootThread {
    pub count: i64,
    pub last_sender: String,
    pub last_ts: i64,
    /// newest first, up to 3 (the avatars)
    pub repliers: Vec<String>,
    pub unread: i64,
    pub mine: bool,
}

pub(crate) fn root_thread(
    root: &Value,
    seen: &HashMap<String, Vec<ReplyRef>>,
    me: &str,
    receipts: &[MyReceipt],
    ts_of: &HashMap<String, i64>,
) -> Option<RootThread> {
    let id = ev_id(root);
    let empty = Vec::new();
    let mine0 = seen.get(id).unwrap_or(&empty);
    let bundle = bundle_of(root);
    let sum = summarize(bundle.as_ref(), mine0, me)?;
    let mut replies: Vec<ReplyRef> = mine0.clone();
    if let Some(b) = &bundle {
        if !replies.iter().any(|r| r.id == b.latest.id) {
            replies.push(b.latest.clone());
        }
    }
    let mut known: Vec<(String, i64)> = ts_of.iter().map(|(i, t)| (i.clone(), *t)).collect();
    known.push((id.to_string(), ev_ts(root)));
    known.extend(replies.iter().map(|r| (r.id.clone(), r.ts)));
    Some(RootThread {
        count: sum.count,
        last_sender: sum.last_sender,
        last_ts: sum.last_ts,
        repliers: sum.repliers,
        unread: thread_unread(id, &known, &replies, me, receipts),
        mine: sum.mine,
    })
}

/// A thread message's m.in_reply_to with is_falling_back is only for clients without threads: no quote.
pub(crate) fn fallback_quote(thread_id: &str, rel: Option<&Value>) -> bool {
    !thread_id.is_empty() && rel.and_then(|r| r.get("is_falling_back")).and_then(|f| f.as_bool()) == Some(true)
}

/// A thread's events for its panel, NEWEST FIRST (the order room.rs's window keeps): the room window's and the fetched
/// ones merged by id (the window's copy wins: it carries live redactions and edits), the root added when given,
/// only the thread's own events (the root, its m.thread replies and what relates to them), by ts then id.
pub(crate) fn merge_thread(root: &str, window: &[Value], fetched: &[Value], root_ev: Option<&Value>) -> Vec<Value> {
    let mut by_id: Vec<Value> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut put = |v: &Value| {
        let id = ev_id(v).to_string();
        if id.is_empty() {
            return;
        }
        match index.get(&id) {
            Some(&i) => by_id[i] = v.clone(),
            None => {
                index.insert(id, by_id.len());
                by_id.push(v.clone());
            }
        }
    };
    for v in fetched {
        put(v);
    }
    if let Some(r) = root_ev {
        put(r);
    }
    for v in window {
        put(v);
    }
    let ids = thread_ids(&by_id);
    let mut keep: Vec<Value> = by_id.into_iter().zip(ids).filter(|(e, t)| in_thread_view(e, t, root)).map(|(e, _)| e).collect();
    keep.sort_by(|a, b| ev_ts(b).cmp(&ev_ts(a)).then_with(|| ev_id(b).cmp(ev_id(a))));
    keep
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ME: &str = "@me:x";

    fn ev(id: &str, sender: &str, ts: i64, content: Value, ty: &str) -> Value {
        json!({ "event_id": id, "sender": sender, "origin_server_ts": ts, "type": ty, "content": content })
    }
    fn msg(body: &str, rel: Option<Value>) -> Value {
        let mut c = json!({ "msgtype": "m.text", "body": body });
        if let Some(r) = rel {
            c["m.relates_to"] = r;
        }
        c
    }
    fn thr(root: &str, prev: &str, fb: bool) -> Value {
        json!({ "rel_type": "m.thread", "event_id": root, "is_falling_back": fb, "m.in_reply_to": { "event_id": prev } })
    }
    // the same fixture as the web's threads.test.ts
    fn root() -> Value {
        let mut r = ev("$root", "@ana:x", 1000, msg("plans for saturday?", None), "m.room.message");
        r["unsigned"] = json!({ "m.relations": { "m.thread": { "count": 2, "current_user_participated": false,
            "latest_event": { "event_id": "$t2", "sender": "@bo:x", "origin_server_ts": 3000, "type": "m.room.message", "content": {} } } } });
        r
    }
    fn t1() -> Value {
        ev("$t1", "@ana:x", 2000, msg("anyone?", Some(thr("$root", "$root", true))), "m.room.message")
    }
    fn t2() -> Value {
        ev("$t2", "@bo:x", 3000, msg("me!", Some(thr("$root", "$t1", true))), "m.room.message")
    }
    fn r1() -> Value {
        ev("$re", "@cy:x", 3100, json!({ "m.relates_to": { "rel_type": "m.annotation", "event_id": "$t2", "key": "👍" } }), "m.reaction")
    }
    fn main_ev() -> Value {
        ev("$main", "@cy:x", 2500, msg("unrelated", None), "m.room.message")
    }
    fn expl() -> Value {
        ev("$t3", "@cy:x", 3200, msg("> <@ana:x> anyone?\n\nyes, explicitly", Some(thr("$root", "$t1", false))), "m.room.message")
    }
    fn rr() -> Value {
        ev("$rr", "@cy:x", 3300, json!({ "m.relates_to": { "rel_type": "m.annotation", "event_id": "$root", "key": "🎉" } }), "m.reaction")
    }
    fn all() -> Vec<Value> {
        vec![root(), t1(), main_ev(), t2(), r1()]
    }
    fn shown(evs: &[Value], view: Option<&str>) -> Vec<String> {
        let p = thread_plan(evs, view);
        evs.iter()
            .zip(p.in_view.iter())
            .filter(|(e, v)| **v && e["type"] == "m.room.message")
            .map(|(e, _)| ev_id(e).to_string())
            .collect()
    }
    fn summary(evs: &[Value], receipts: &[MyReceipt]) -> RootThread {
        let p = thread_plan(evs, None);
        root_thread(&root(), &p.seen, ME, receipts, &p.ts_of).unwrap()
    }
    fn rc(thread: &str, id: &str, ts: i64) -> MyReceipt {
        MyReceipt { thread: thread.into(), event_id: id.into(), ts }
    }

    #[test]
    fn main_view_hides_thread_replies_and_summarises_the_root() {
        assert_eq!(shown(&all(), None), vec!["$root", "$main"]);
        let p = thread_plan(&all(), None);
        assert_eq!(p.tids, vec!["", "$root", "", "$root", "$root"], "the reaction on $t2 is in the thread too");
        let t = summary(&all(), &[]);
        assert_eq!(t.count, 2);
        assert_eq!(t.last_sender, "@bo:x");
        assert_eq!(t.last_ts, 3000);
        assert_eq!(t.repliers, vec!["@bo:x", "@ana:x"]);
        assert_eq!(t.unread, 2, "nothing read yet: both replies by others are unread");
        assert!(root_thread(&main_ev(), &p.seen, ME, &[], &p.ts_of).is_none(), "no thread on a plain message");
    }

    #[test]
    fn a_reply_newer_than_the_bundle_adds_to_its_count() {
        let mut evs = all();
        evs.push(ev("$t4", "@dee:x", 4000, msg("late", Some(thr("$root", "$t2", true))), "m.room.message"));
        let t = summary(&evs, &[]);
        assert_eq!(t.count, 3);
        assert_eq!(t.last_sender, "@dee:x");
    }

    #[test]
    fn unread_follows_msc3771() {
        assert_eq!(summary(&all(), &[rc("$root", "$t2", 9999)]).unread, 0);
        assert_eq!(summary(&all(), &[rc("main", "$main", 9999)]).unread, 2);
        assert_eq!(summary(&all(), &[rc("", "$main", 9999)]).unread, 1, "an unthreaded receipt reads up to its (loaded) event's ts, 2500: only $t2 stays unread");
        assert_eq!(summary(&all(), &[rc("", "$gone", 9999)]).unread, 2, "an unthreaded receipt on an event not loaded says nothing");
        assert_eq!(summary(&all(), &[rc("$root", "$gone", 9999)]).unread, 0, "the thread's own receipt on an event not loaded counts by its ts");
        assert_eq!(summary(&all(), &[rc("", "$t1", 9999)]).unread, 1, "a receipt on a known thread event reads up to that event's ts");
    }

    #[test]
    fn thread_view_root_then_replies_and_fallback_quotes() {
        let mut evs = all();
        evs.push(expl());
        evs.push(rr());
        assert_eq!(shown(&evs, Some("$root")), vec!["$root", "$t1", "$t2", "$t3"]);
        let p = thread_plan(&evs, Some("$root"));
        assert!(p.in_view[4], "the reaction on $t2 rides along");
        assert!(p.in_view[6], "and the root's own reaction (review #9)");
        assert!(!p.in_view[2], "never a main message");
        assert!(fallback_quote("$root", t2()["content"].get("m.relates_to")), "is_falling_back: no quote");
        assert!(!fallback_quote("$root", expl()["content"].get("m.relates_to")), "is_falling_back false: the quote stays");
        assert!(!fallback_quote("", Some(&json!({ "m.in_reply_to": { "event_id": "$x" } }))), "a main reply always quotes");
    }

    #[test]
    fn old_clients_see_a_normal_reply() {
        // what Tin Hat 12.1.15 and other clients without threads show: the spec's fallback makes a thread reply a normal
        // reply to the message before it in the thread
        assert_eq!(t2()["content"]["m.relates_to"]["m.in_reply_to"], json!({ "event_id": "$t1" }));
    }

    #[test]
    fn merge_thread_dedupes_sorts_and_keeps_only_the_thread() {
        let fetched_t1 = ev("$t1", "@ana:x", 2000, msg("anyone? (fetched)", Some(thr("$root", "$root", true))), "m.room.message");
        let old = ev("$t0", "@ana:x", 1500, msg("older, fetched", Some(thr("$root", "$root", true))), "m.room.message");
        let out = merge_thread("$root", &all(), &[fetched_t1, old, rr()], None);
        let ids: Vec<&str> = out.iter().map(ev_id).collect();
        assert_eq!(ids, vec!["$rr", "$re", "$t2", "$t1", "$t0", "$root"], "newest first");
        assert_eq!(out[3]["content"]["body"], "anyone?", "the window's copy of $t1 wins");
    }
}
