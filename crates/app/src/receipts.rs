//! Every read receipt and read marker the app sends goes through this file (the one guarded decision). The web's
//! `src/core/matrix/receipts.ts` is the same; the pure rule is replayed from `testvectors/notifactions.json`
//! (`receiptRule`, byte-identical in both repos; the web owns it).
//!
//! THE RULE (fix/notif-actions-foreign-receipts, 2026-11-07): a room is "mine" only when
//! `foreign_room::is_managed(create, own_server)` holds (the create sender and every v12 `additional_creators` entry
//! on my own server). Mine → a public `m.read`, as always. Anything else, a missing create event (unknown) included →
//! a private `m.read.private` (plus the `m.fully_read` marker wherever the public path sends one): a public receipt
//! there would tell another server's people when I read.
//! Fail-private on purpose: rooms on connected Tin Hat servers get private receipts too on this branch. The open-network
//! work (slice 1) narrows that later with its connected-servers list; until then, never widen this check here.
//!
//!   public_receipt_ok    the pure decision (create event or none, my server) → may this room get a public m.read?
//!   markers_body         the `POST /rooms/{room}/read_markers` body for one event under that decision
//!   send_read_markers    fully-read marker + receipt on one event (Mark as read: notification, room list, DM strip)
//!   send_read_receipt    the receipt alone (the open room's reading position); threads (MSC3771): with a thread —
//!                        "main" for the main timeline, a root for a thread's panel; never an unthreaded fallback
//!   own_read_up_to       where my own receipt sits, public or private, unthreaded or "main" (the unread divider)
//!   my_thread_receipts   threads: my receipts that count for these threads (unthreaded + each thread's), for the
//!                        unread dots (tinhat_widgets::threads::thread_unread)
//!
//! `tests::every_receipt_send_goes_through_receipts_rs` walks crates/ and fails on any send outside this file.

use matrix_sdk::Room;
use serde_json::{json, Value};

/// The receipt key a room gets: `m.read` only when it is mine (see THE RULE above).
pub(crate) fn receipt_key(public_ok: bool) -> &'static str {
    if public_ok {
        "m.read"
    } else {
        "m.read.private"
    }
}

/// THE RULE as one pure decision: None (no create event known) → false; else `is_managed`.
pub(crate) fn public_receipt_ok(create: Option<&Value>, own_server: &str) -> bool {
    create.is_some_and(|c| !c.is_null() && crate::foreign_room::is_managed(c, own_server))
}

/// The read_markers body for `event`: `m.fully_read` plus the receipt key [`receipt_key`] picks.
pub(crate) fn markers_body(event: &str, public_ok: bool) -> Value {
    let mut body = json!({ "m.fully_read": event });
    body[receipt_key(public_ok)] = Value::from(event);
    body
}

/// [`public_receipt_ok`] for a room in the local store: exactly `foreign_room::managed_here` (the store read the
/// power-level guard uses; my server = my user id's; no create state → false = private), so the two can't drift.
pub(crate) async fn public_receipt_here(room: &Room) -> bool {
    crate::foreign_room::managed_here(room).await
}

/// The fully-read marker + the receipt [`public_receipt_here`] allows, on `event`, in one `read_markers` call.
pub(crate) async fn send_read_markers(room: &Room, event: &str) -> Result<(), String> {
    use matrix_sdk::room::Receipts;
    let eid = matrix_sdk::ruma::OwnedEventId::try_from(event).map_err(|e| e.to_string())?;
    let public_ok = public_receipt_here(room).await;
    let body = markers_body(event, public_ok);
    // the SDK call is built from the vector-tested body, key by key
    let mut receipts = Receipts::new().fully_read_marker(eid.clone());
    if body.get("m.read").is_some() {
        receipts = receipts.public_read_receipt(eid);
    } else if body.get("m.read.private").is_some() {
        receipts = receipts.private_read_receipt(eid);
    }
    room.send_multiple_receipts(receipts).await.map_err(|e| e.to_string())
}

/// threads: the receipt thread for `thread` ("" = unthreaded, "main", or a thread's root).
pub(crate) fn receipt_thread(thread: &str) -> Result<matrix_sdk::ruma::events::receipt::ReceiptThread, String> {
    use matrix_sdk::ruma::events::receipt::ReceiptThread;
    Ok(match thread {
        "" => ReceiptThread::Unthreaded,
        "main" => ReceiptThread::Main,
        root => ReceiptThread::Thread(matrix_sdk::ruma::OwnedEventId::try_from(root).map_err(|e| e.to_string())?),
    })
}

/// The receipt alone, public or private as [`public_receipt_here`] says. threads (MSC3771, spec v1.4): `thread` = ""
/// (unthreaded), "main" (the main timeline: reading it never clears a thread) or a thread's root (its panel). A refused
/// threaded receipt (400: most likely an event the server files under another thread) is NOT retried unthreaded —
/// that would clear every thread (review 2026-11-13); the next read tries again.
pub(crate) async fn send_read_receipt(room: &Room, event: &matrix_sdk::ruma::EventId, thread: &str) -> Result<(), String> {
    use matrix_sdk::ruma::api::client::receipt::create_receipt::v3::ReceiptType as SendReceiptType;
    let ty = if public_receipt_here(room).await { SendReceiptType::Read } else { SendReceiptType::ReadPrivate };
    room.send_single_receipt(ty, receipt_thread(thread)?, event.to_owned()).await.map_err(|e| {
        eprintln!("[threads] the server refused a {} receipt: {e}", if thread.is_empty() { "unthreaded" } else if thread == "main" { "main" } else { "thread" });
        e.to_string()
    })
}

/// threads: my receipts that count for `roots` (core thread_unread): the unthreaded one and each thread's, public and
/// private, with each receipt's own time. Local store reads only.
pub(crate) async fn my_thread_receipts(room: &Room, roots: &[String]) -> Vec<tinhat_widgets::threads::MyReceipt> {
    use matrix_sdk::ruma::events::receipt::ReceiptType;
    let own = room.own_user_id();
    let mut out = Vec::new();
    let mut threads: Vec<&str> = vec![""];
    threads.extend(roots.iter().map(|r| r.as_str()));
    for t in threads {
        let Ok(th) = receipt_thread(t) else { continue };
        for ty in [ReceiptType::Read, ReceiptType::ReadPrivate] {
            if let Ok(Some((eid, r))) = room.load_user_receipt(ty, &th, own).await {
                let ts = r.ts.map(|x| i64::from(x.0)).unwrap_or(0);
                out.push(tinhat_widgets::threads::MyReceipt { event_id: eid.to_string(), ts, thread: t.to_string() });
            }
        }
    }
    out
}

/// Where my own unthreaded receipt sits: of my public and private receipts, the one on the later event ("" = none
/// known). A room that gets private receipts must still move the unread divider.
/// Order: [`later_receipt`] on the two receipted events' `origin_server_ts` (both fetched at once, one 2 s deadline:
/// this runs on room open). That is the sending servers' clocks, not the timeline position: skew between servers can
/// misorder two events seconds apart. A receipt's own time is never used (a receipt sent later can point at an OLDER
/// event: Mark as read on an old notification); a tie or an event that can't be loaded goes to the receipt this
/// room's rule sends now (public in a room of my own server, else private), the one that keeps moving.
/// 12.1.17 (notify.rs, the quiet Memories photos' ledger): is this m.receipt content (a sync batch's) one of mine on the
/// main timeline (unthreaded or "main"; public or private)? A read, never a send.
pub(crate) fn has_own_main_receipt(content: &Value, me: &str) -> bool {
    let Some(events) = content.as_object() else { return false };
    events.values().any(|per_type| {
        ["m.read", "m.read.private"].iter().any(|ty| {
            per_type.get(*ty).and_then(|u| u.get(me)).is_some_and(|r| matches!(r.get("thread_id").and_then(|t| t.as_str()), None | Some("main")))
        })
    })
}

pub(crate) async fn own_read_up_to(room: &Room) -> String {
    use matrix_sdk::ruma::events::receipt::{ReceiptThread, ReceiptType};
    let own = room.own_user_id();
    // threads (MSC3771): the main timeline's receipts are "main" now; an unthreaded one (older apps, Mark as read) counts
    // too — per type, the later of the two (by the same event-time rule)
    let load_one = |ty, th| async move { room.load_user_receipt(ty, &th, own).await.ok().flatten().map(|(e, _)| e) };
    let load = |ty: ReceiptType| async move {
        let un = load_one(ty.clone(), ReceiptThread::Unthreaded).await;
        let main = load_one(ty, ReceiptThread::Main).await;
        match (un, main) {
            (Some(a), Some(b)) if a != b => {
                let ts = |id: matrix_sdk::ruma::OwnedEventId| async move {
                    let ev = Box::pin(room.load_or_fetch_event(&id, None)).await.ok()?;
                    ev.raw().get_field::<u64>("origin_server_ts").ok().flatten()
                };
                let (ta, tb) = tokio::time::timeout(std::time::Duration::from_secs(2), async { tokio::join!(ts(a.clone()), ts(b.clone())) })
                    .await
                    .unwrap_or((None, None));
                // the main receipt is the one this app keeps moving: it wins a tie or an unknown time
                Some(if later_receipt(ta, tb, false) { a } else { b })
            }
            (a, b) => b.or(a),
        }
    };
    let (public, private) = (load(ReceiptType::Read).await, load(ReceiptType::ReadPrivate).await);
    let (public, private) = match (public, private) {
        (None, None) => return String::new(),
        (Some(e), None) | (None, Some(e)) => return e.to_string(),
        (Some(a), Some(b)) if a == b => return a.to_string(),
        (Some(a), Some(b)) => (a, b),
    };
    let event_ts = |id: matrix_sdk::ruma::OwnedEventId| async move {
        let ev = Box::pin(room.load_or_fetch_event(&id, None)).await.ok()?;
        ev.raw().get_field::<u64>("origin_server_ts").ok().flatten()
    };
    let both = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(event_ts(public.clone()), event_ts(private.clone()))
    })
    .await
    .unwrap_or((None, None));
    let prefer_public = public_receipt_here(room).await;
    if later_receipt(both.0, both.1, prefer_public) { public.to_string() } else { private.to_string() }
}

/// true = the public receipt is the later one: by the events' times; a tie or an unknown time → `prefer_public`.
pub(crate) fn later_receipt(public_ts: Option<u64>, private_ts: Option<u64>, prefer_public: bool) -> bool {
    match (public_ts, private_ts) {
        (Some(a), Some(b)) if a != b => a > b,
        _ => prefer_public,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_main_receipts_in_a_sync_batch_12_1_17() {
        let me = "@me:x";
        assert!(has_own_main_receipt(&serde_json::json!({"$e": {"m.read": {"@me:x": {"ts": 1}}}}), me));
        assert!(has_own_main_receipt(&serde_json::json!({"$e": {"m.read.private": {"@me:x": {"ts": 1, "thread_id": "main"}}}}), me));
        assert!(!has_own_main_receipt(&serde_json::json!({"$e": {"m.read": {"@me:x": {"ts": 1, "thread_id": "$root"}}}}), me));
        assert!(!has_own_main_receipt(&serde_json::json!({"$e": {"m.read": {"@you:x": {"ts": 1}}}}), me));
        assert!(!has_own_main_receipt(&serde_json::json!([]), me));
    }

    #[test]
    fn the_divider_takes_the_later_event_never_the_later_receipt() {
        // a later event wins whatever order the receipts were sent in
        assert!(later_receipt(Some(20), Some(10), false));
        assert!(!later_receipt(Some(10), Some(20), true));
        // a tie or an event that couldn't be loaded: the receipt this room's rule sends now
        assert!(later_receipt(Some(10), Some(10), true));
        assert!(!later_receipt(Some(10), Some(10), false));
        assert!(!later_receipt(None, Some(10), false));
        assert!(later_receipt(Some(10), None, true));
        assert!(!later_receipt(None, None, false));
    }

    #[test]
    fn the_rule() {
        let own = "home.example";
        assert!(public_receipt_ok(Some(&json!({"sender": "@a:home.example", "content": {}})), own));
        assert!(!public_receipt_ok(Some(&json!({"sender": "@a:friend.example", "content": {}})), own));
        assert!(!public_receipt_ok(None, own));
        assert!(!public_receipt_ok(Some(&Value::Null), own));
        assert!(!public_receipt_ok(Some(&json!({"sender": "@a:home.example", "content": {}})), ""));
        assert_eq!(markers_body("$e", true), json!({"m.fully_read": "$e", "m.read": "$e"}));
        assert_eq!(markers_body("$e", false), json!({"m.fully_read": "$e", "m.read.private": "$e"}));
    }

    /// Code with comment lines removed (`//`, `///`, `//!`), flattened to one whitespace-free string, so a call split
    /// over lines is still seen.
    fn flat(text: &str) -> String {
        text.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join(" ").split_whitespace().collect()
    }

    /// Anything that can put a receipt or read marker on the wire.
    const SENDS: &[&str] = &[
        // method references and field writes count, not only calls (`let f = Room::send_multiple_receipts;`,
        // `receipts.public_read_receipt = Some(e)`)
        "send_single_receipt",
        "send_multiple_receipts",
        "public_read_receipt",
        "private_read_receipt",
        "fully_read_marker",
        "Receipts::new",
        "Receipts::default",
        "Receipts{",
        "set_read_marker(",
        "read_marker::set_read_marker",
        "receipt::create_receipt",
        "create_receipt::v3",
        "mark_as_read",
        "send_read_receipt(",
        "/receipt/",
        "/read_markers",
        "\"m.read\"",
        "\"m.read.private\"",
        "\"m.fully_read\"",
    ];

    /// Files that may say `"m.read"` / `"m.fully_read"` / `send_read_receipt(` without sending anything themselves:
    /// notif_actions.rs builds its vector body through [`markers_body`] and calls this file's senders.
    const CALLERS: &[&str] = &["notif_actions.rs"];

    /// The receipt READS (`ReceiptType::Read` on the event side), counted per file. A new one fails until it is
    /// reviewed here: loading a receipt is fine, sending one is not.
    // room.rs 2: attach_read_receipts loads each row's readers, and (12.1.17 round 2, review #8) the readers of a photo the
    // chat hides — both LOADS (load_event_receipts), never a send
    const READS: &[(&str, usize)] = &[("notify.rs", 1), ("room.rs", 2)];

    fn rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            if p.is_dir() {
                if name != "target" && !name.starts_with('.') {
                    rs_files(&p, out);
                }
            } else if name.ends_with(".rs") {
                out.push(p);
            }
        }
    }

    /// The receipt audit (THE RULE above): every receipt / read-marker send in crates/ is in this file, behind
    /// [`public_receipt_here`]; a public receipt can't be sent from anywhere else.
    #[test]
    fn every_receipt_send_goes_through_receipts_rs() {
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
        let mut files = Vec::new();
        rs_files(&crates, &mut files);
        assert!(files.len() > 20, "the walk found {} files under {}", files.len(), crates.display());
        let me = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/receipts.rs");
        let mut offenders = Vec::new();
        let mut reads = std::collections::BTreeMap::new();
        for p in &files {
            if *p == me {
                continue;
            }
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            let code = flat(&std::fs::read_to_string(p).unwrap_or_default());
            for s in SENDS {
                let s_flat: String = s.split_whitespace().collect();
                if !code.contains(&s_flat) {
                    continue;
                }
                let caller_ok = CALLERS.contains(&name.as_str())
                    && matches!(*s, "\"m.read\"" | "\"m.read.private\"" | "\"m.fully_read\"");
                // a call of this file's own sender is the guarded path
                let via_here = *s == "send_read_receipt(" && !code.replace("crate::receipts::send_read_receipt(", "").contains("send_read_receipt(");
                if !caller_ok && !via_here {
                    offenders.push(format!("{}: {s}", p.strip_prefix(&crates).unwrap_or(p).display()));
                }
            }
            let n = code.matches("ReceiptType::Read,").count() + code.matches("ReceiptType::Read)").count() + code.matches("ReceiptType::Read]").count();
            if n > 0 {
                reads.insert(name, n);
            }
        }
        assert!(offenders.is_empty(), "receipt sends outside receipts.rs (route them through it): {offenders:?}");
        let want: std::collections::BTreeMap<String, usize> = READS.iter().map(|(f, n)| (f.to_string(), *n)).collect();
        assert_eq!(reads, want, "ReceiptType::Read outside receipts.rs changed: review it (a load is fine, a send is not)");
        // this file: every sender asks public_receipt_here, and only the guarded branch names a public receipt
        let here = flat(&std::fs::read_to_string(&me).unwrap());
        let code = here.split("#[cfg(test)]").next().unwrap();
        assert_eq!(code.matches("public_receipt_here(room)").count(), 3, "both senders (and the divider tie-break) decide through public_receipt_here");
        assert_eq!(code.matches(".public_read_receipt(").count(), 1, "one public read receipt, in the guarded branch");
        assert_eq!(code.matches("SendReceiptType::Read}").count(), 1, "one public single receipt, in the guarded branch");
        assert_eq!(code.matches("send_multiple_receipts(").count() + code.matches("send_single_receipt(").count(), 2);
    }
}
