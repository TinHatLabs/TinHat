//! PeopleController — the add-people picker's data (2026-11-12, feat/people-search): room Invite, New DM and space
//! Invite all use qml/PeoplePicker.qml, which owns one of these. The rules are people.rs (shared vectors with Tin Hat
//! Web's people.ts); this file only gathers the people (the web's source.ts does the same three things):
//!   People you know   everyone you share a joined room or DM with, read from the local store once per open (no
//!                     network, every server); a DM = `room.is_direct()` (m.direct)
//!   the directory     your server's user directory, 10 results, for the text QML hands `search()` (QML debounces it
//!                     250 ms; an answer for older text is dropped)
//!   a look-up         YOUR homeserver's profile look-up for a typed username/address nobody listed (your server asks
//!                     theirs over federation), status + errcode → people::lookup_outcome's plain words
//! Members of the room/space (joined or invited) are dimmed; someone invited here is added to them once the server said
//! yes (invite(); a failure keeps them pickable with the reason, 2026-11-13).

use core::pin::Pin;
use std::collections::{HashMap, HashSet};

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use serde_json::{json, Value};

use crate::people::{empty_note, invite_failure, invite_step, InviteEvent, Sent, lookup_outcome, rank_people, section_title, DirectoryHit, KnownPerson, LookupKind, Outcome, SearchState, DIRECTORY_MAX};

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
        #[qproperty(QString, rows_json)] // QML: rowsJson — [{section, head, mxid, name, avatar, server, line, member, lookup, look, note, pickable, anyway, dm}]
        #[qproperty(i32, pick)] //          QML: pick — Enter's row (-1 = none): the typed person first
        #[qproperty(i32, fallback)] //      QML: fallback — Enter once the typed person's look-up failed (your server only)
        #[qproperty(QString, kind)] //      QML: kind — what the text is: empty / name / username / address
        #[qproperty(QString, me)] //        QML: me — your own address (the "Copy my address" footer)
        #[qproperty(QString, own_server)] // QML: ownServer
        #[qproperty(QString, empty_note)] // QML: emptyNote — the empty list's line (what to type / Searching… / failed)
        #[qproperty(bool, search_failed)] // QML: searchFailed — the directory search failed (Try again)
        #[qproperty(bool, retry_pending)] // QML: retryPending — a look-up worth another try is on screen (QML's timer)
        type PeopleController = super::PeopleControllerRust;

        /// QML: open(roomId) — a fresh picker; roomId's members are dimmed ("" = none, New DM).
        #[qinvokable]
        fn open(self: Pin<&mut PeopleController>, room_id: &QString);
        /// QML: setText(text) — re-rank now (People you know is instant; the directory waits for search()).
        #[qinvokable]
        fn set_text(self: Pin<&mut PeopleController>, text: &QString);
        /// QML: search() — the directory (and a look-up row's look-up) for the current text.
        #[qinvokable]
        fn search(self: Pin<&mut PeopleController>);
        /// QML: picked(mxid) — they were invited here: dim them from now on.
        #[qinvokable]
        fn picked(self: Pin<&mut PeopleController>, mxid: &QString);
        /// QML: invite(roomId, mxid, name) — invite them to the room/space (2026-11-13, A02.0): "Inviting…" until the
        /// answer; a success dims them; a failure keeps them pickable (pick again = try again) with the reason. Ends
        /// with invited().
        #[qinvokable]
        fn invite(self: Pin<&mut PeopleController>, room_id: &QString, mxid: &QString, name: &QString);
        /// QML: retry() — Try again: the directory search and a failed look-up worth another try, now.
        #[qinvokable]
        fn retry(self: Pin<&mut PeopleController>);

        /// The answer to invite(): ok + the words to show (plain words when it failed).
        #[qsignal]
        fn invited(self: Pin<&mut PeopleController>, mxid: QString, ok: bool, text: QString);
    }

    impl cxx_qt::Threading for PeopleController {}
}

#[derive(Clone)]
enum Looked {
    Looking,
    Done { outcome: Outcome, name: String, avatar: String, at: std::time::Instant },
}

/// Look-ups worth another try (after RETRY, at the next search()).
const RETRY: std::time::Duration = std::time::Duration::from_secs(5);
fn retryable(o: &Outcome) -> bool {
    matches!(o.kind, LookupKind::Unreachable | LookupKind::RateLimited)
}

#[derive(Default)]
pub struct PeopleControllerRust {
    rows_json: QString,
    pick: i32,
    fallback: i32,
    kind: QString,
    me: QString,
    own_server: QString,
    empty_note: QString,
    search_failed: bool,
    retry_pending: bool,
    text: String,
    room_id: String,
    known: Vec<KnownPerson>,
    members: Vec<String>,
    dir_term: String,
    dir_hits: Vec<DirectoryHit>,
    /// the directory search for `dir_term` failed
    dir_failed: bool,
    sent: HashMap<String, Sent>,
    /// look-ups on the wire (a retry never starts a second one for the same person)
    lookup_inflight: HashSet<String>,
    looked: HashMap<String, Looked>,
    /// mxc → a local file url ("" = failed)
    avatars: HashMap<String, String>,
    avatar_pending: HashSet<String>,
    open_seq: u64,
    search_seq: u64,
}

impl qobject::PeopleController {
    pub fn open(mut self: Pin<&mut Self>, room_id: &QString) {
        let Some(client) = crate::session::client() else { return };
        let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
        let own = crate::dm_target::server_of(&me).to_string();
        let seq = self.rust().open_seq.wrapping_add(1);
        {
            // a fresh picker; the generations stay monotonic, so no answer from an earlier opening can land in this one
            let mut r = self.as_mut().rust_mut();
            let keep = (std::mem::take(&mut r.avatars), r.search_seq.wrapping_add(1));
            *r = PeopleControllerRust::default();
            r.avatars = keep.0;
            r.search_seq = keep.1;
            r.open_seq = seq;
            r.room_id = room_id.to_string();
        }
        self.as_mut().set_me(QString::from(me.as_str()));
        self.as_mut().set_own_server(QString::from(own.as_str()));
        self.as_mut().rebuild();
        let rid = room_id.to_string();
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let (known, members) = load(&client, &me, &rid).await;
            let _ = qt.queue(move |mut o| {
                if o.rust().open_seq != seq {
                    return;
                }
                {
                    let mut r = o.as_mut().rust_mut();
                    r.known = known;
                    for m in members {
                        if !r.members.contains(&m) {
                            r.members.push(m);
                        }
                    }
                }
                o.as_mut().rebuild();
            });
        });
    }

    pub fn set_text(mut self: Pin<&mut Self>, text: &QString) {
        {
            let mut r = self.as_mut().rust_mut();
            r.text = text.to_string();
            r.search_seq = r.search_seq.wrapping_add(1); // a directory answer in flight is for older text
        }
        self.as_mut().rebuild();
    }

    pub fn search(mut self: Pin<&mut Self>) {
        let Some(client) = crate::session::client() else { return };
        let term = crate::people::atrim(&self.rust().text).to_string();
        let seq = self.rust().search_seq.wrapping_add(1);
        self.as_mut().rust_mut().search_seq = seq;
        let opened = self.rust().open_seq;
        if term.is_empty() {
            return;
        }
        let qt = self.qt_thread();
        let c = client.clone();
        let t = term.clone();
        crate::bridge::runtime().spawn(async move {
            let hits: Option<Vec<DirectoryHit>> = match c.search_users(&t, DIRECTORY_MAX as u64).await {
                Ok(r) => Some(
                    r.results
                        .into_iter()
                        .map(|u| DirectoryHit {
                            mxid: u.user_id.to_string(),
                            name: u.display_name.unwrap_or_default(),
                            avatar: u.avatar_url.map(|m| m.to_string()).unwrap_or_default(),
                        })
                        .collect(),
                ),
                Err(e) => {
                    eprintln!("[people] directory search failed: {e}");
                    None
                }
            };
            let _ = qt.queue(move |mut o| {
                if o.rust().search_seq != seq || o.rust().open_seq != opened {
                    return; // a newer text, or a newer opening
                }
                {
                    let mut r = o.as_mut().rust_mut();
                    r.dir_term = t;
                    r.dir_failed = hits.is_none();
                    r.dir_hits = hits.unwrap_or_default();
                }
                o.as_mut().rebuild();
            });
        });
        // the look-up row (a typed username/address nobody listed): your server asks for their profile, once
        let ranked = self.ranked();
        if let Some(target) = ranked.rows.iter().find(|r| r.lookup).map(|r| r.mxid.clone()) {
            if self.rust().lookup_inflight.contains(&target) {
                return;
            }
            match self.rust().looked.get(&target) {
                None => {}
                Some(Looked::Done { outcome, at, .. }) if retryable(outcome) && at.elapsed() > RETRY => {}
                Some(_) => return,
            }
            // a retry keeps the earlier failure (and its block) until the new answer is in
            let retry = match self.as_mut().rust_mut().looked.get_mut(&target) {
                Some(Looked::Done { at, .. }) => {
                    *at = std::time::Instant::now();
                    true
                }
                _ => false,
            };
            if !retry {
                self.as_mut().rust_mut().looked.insert(target.clone(), Looked::Looking);
            }
            self.as_mut().rust_mut().lookup_inflight.insert(target.clone());
            self.as_mut().rebuild();
            let own = self.own_server().to_string();
            let qt = self.qt_thread();
            crate::bridge::runtime().spawn(async move {
                let (status, errcode, message, name, avatar) = lookup_profile(&client, &target).await;
                let outcome = lookup_outcome(status, &errcode, &message, &target, &own);
                eprintln!("[people] profile look-up: {status} {errcode} → {}", outcome.kind.as_str());
                let _ = qt.queue(move |mut o| {
                    if o.rust().open_seq != opened {
                        return; // the picker was opened again since
                    }
                    let at = std::time::Instant::now();
                    o.as_mut().rust_mut().lookup_inflight.remove(&target);
                    o.as_mut().rust_mut().looked.insert(target, Looked::Done { outcome, name, avatar, at });
                    o.as_mut().rebuild();
                });
            });
        }
    }

    pub fn picked(mut self: Pin<&mut Self>, mxid: &QString) {
        let m = mxid.to_string();
        if !self.rust().members.contains(&m) {
            self.as_mut().rust_mut().members.push(m);
        }
        self.as_mut().rebuild();
    }

    pub fn invite(mut self: Pin<&mut Self>, room_id: &QString, mxid: &QString, name: &QString) {
        let id = mxid.to_string();
        if matches!(self.rust().sent.get(&id), Some(Sent::Inviting)) || self.rust().members.contains(&id) {
            return;
        }
        let Some(client) = crate::session::client() else { return };
        let (rid, shown, own, opened) = (room_id.to_string(), name.to_string(), self.own_server().to_string(), self.rust().open_seq);
        // people.rs invite_step: in flight, not yet a member
        let (_, sent) = invite_step(&self.rust().members, &self.rust().sent, &id, InviteEvent::Start);
        self.as_mut().rust_mut().sent = sent;
        self.as_mut().rebuild();
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let res: Result<(), String> = match (matrix_sdk::ruma::RoomId::parse(&rid), matrix_sdk::ruma::UserId::parse(&id)) {
                (Ok(r), Ok(u)) => match client.get_room(&r) {
                    Some(room) => room.invite_user_by_id(&u).await.map_err(|e| {
                        let (status, errcode, message) = crate::people::error_parts(&e);
                        eprintln!("[people] invite failed: {status} {errcode}");
                        invite_failure(status, &errcode, &message, &id, &own, false).1
                    }),
                    None => Err("That room isn't loaded yet".to_string()),
                },
                _ => Err(format!("{id} isn't a username or an address like @name:their.server")),
            };
            let _ = qt.queue(move |mut o| {
                let (ok, text) = match res {
                    Ok(()) => (true, format!("Invited {}", if shown.trim().is_empty() { id.as_str() } else { shown.trim() })),
                    Err(t) => (false, t),
                };
                if o.rust().open_seq != opened {
                    // the picker was opened again since: its rows are someone else's now, but the answer is still said
                    // (with no person: it must not touch the new opening's field or choice)
                    o.as_mut().invited(QString::default(), ok, QString::from(text.as_str()));
                    return;
                }
                // people.rs invite_step: only a yes makes them a member (dimmed); a no keeps them pickable with the reason
                let (members, sent) = invite_step(&o.rust().members, &o.rust().sent, &id, InviteEvent::Answer { ok, text: &text });
                {
                    let mut r = o.as_mut().rust_mut();
                    r.members = members;
                    r.sent = sent;
                }
                o.as_mut().rebuild();
                o.as_mut().invited(QString::from(id.as_str()), ok, QString::from(text.as_str()));
            });
        });
    }

    pub fn retry(mut self: Pin<&mut Self>) {
        // a failed look-up worth another try goes again now (search() only retries one older than RETRY)
        // "Searching…" again until the new directory answer is in
        self.as_mut().rust_mut().dir_term.clear();
        let old = std::time::Instant::now().checked_sub(RETRY * 2);
        if let Some(old) = old {
            for l in self.as_mut().rust_mut().looked.values_mut() {
                if let Looked::Done { outcome, at, .. } = l {
                    if retryable(outcome) {
                        *at = old;
                    }
                }
            }
        }
        self.search();
    }

    fn ranked(&self) -> crate::people::Ranked {
        let r = self.rust();
        let own = self.own_server().to_string();
        let me = self.me().to_string();
        let hits: &[DirectoryHit] = if r.dir_term == crate::people::atrim(&r.text) && !r.dir_failed { &r.dir_hits } else { &[] };
        rank_people(&r.text, &own, &me, &r.known, hits, &r.members)
    }

    /// The rows QML draws: people.rs's ranking + what each look-up found + pictures as local files.
    fn rebuild(mut self: Pin<&mut Self>) {
        let ranked = self.ranked();
        let own = self.own_server().to_string();
        let mut want_avatars: Vec<String> = Vec::new();
        let mut rows: Vec<Value> = Vec::with_capacity(ranked.rows.len());
        let mut last = None;
        let mut retry_pending = false;
        for row in &ranked.rows {
            if row.lookup {
                if let Some(Looked::Done { outcome, .. }) = self.rust().looked.get(&row.mxid) {
                    retry_pending |= retryable(outcome);
                }
            }
            let sent = if row.member { None } else { self.rust().sent.get(&row.mxid).cloned() };
            let (name, mxc, note, pickable, anyway, look) = if row.member {
                (row.name.clone(), row.avatar.clone(), String::new(), false, false, "")
            } else if let Some(sent) = sent {
                // an invite from this picker: in flight, or failed with the reason (pick again = try again)
                let (shown, pic) = match self.rust().looked.get(&row.mxid) {
                    Some(Looked::Done { name, avatar, .. }) if row.lookup => (if name.trim().is_empty() { row.name.clone() } else { name.trim().to_string() }, avatar.clone()),
                    _ => (row.name.clone(), row.avatar.clone()),
                };
                // the same block as below: a failed look-up still wins over a failed invite (the web's pickable())
                let blocked = match self.rust().looked.get(&row.mxid) {
                    Some(Looked::Done { outcome, .. }) => !outcome.pickable && (row.lookup || !retryable(outcome)),
                    // a look-up row is pickable only once its look-up said yes (the web's pickable())
                    _ => row.lookup,
                };
                match sent {
                    Sent::Inviting => (shown, pic, "Inviting…".to_string(), false, false, "inviting"),
                    Sent::Failed(t) => (shown, pic, t, !blocked, false, "failed"),
                }
            } else if !row.lookup {
                // a failed look-up blocks that person in every row (a directory or room row can't re-enable them), except
                // a failure worth another try (their server was busy or unreachable): a listed row proves they exist
                match self.rust().looked.get(&row.mxid) {
                    Some(Looked::Done { outcome, .. }) if !outcome.pickable && !retryable(outcome) => {
                        (row.name.clone(), row.avatar.clone(), outcome.text.clone(), false, false, outcome.kind.as_str())
                    }
                    _ => (row.name.clone(), row.avatar.clone(), String::new(), true, false, ""),
                }
            } else {
                match self.rust().looked.get(&row.mxid) {
                    Some(Looked::Done { outcome, name, avatar, .. }) => (
                        if name.trim().is_empty() { row.name.clone() } else { name.trim().to_string() },
                        avatar.clone(),
                        outcome.text.clone(),
                        outcome.pickable,
                        outcome.kind == LookupKind::Refused,
                        outcome.kind.as_str(),
                    ),
                    _ => (row.name.clone(), String::new(), "Looking them up…".to_string(), false, false, "looking"),
                }
            };
            let avatar = if mxc.starts_with("mxc://") {
                match self.rust().avatars.get(&mxc) {
                    Some(url) => url.clone(),
                    None => {
                        want_avatars.push(mxc.clone());
                        String::new()
                    }
                }
            } else {
                String::new()
            };
            let head = if last != Some(row.section) { section_title(row.section, &own) } else { String::new() };
            last = Some(row.section);
            let line = if row.server.is_empty() { name.clone() } else { format!("{name} · {}", row.server) };
            rows.push(json!({
                "section": row.section, "head": head, "mxid": row.mxid, "name": name, "avatar": avatar,
                "server": row.server, "line": line, "member": row.member, "lookup": row.lookup, "look": look,
                "note": note, "pickable": pickable, "anyway": anyway, "dm": row.dm,
            }));
        }
        let json = Value::Array(rows).to_string();
        let kind = serde_json::to_value(ranked.query.kind).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
        let search = {
            let r = self.rust();
            let t = crate::people::atrim(&r.text);
            if t.is_empty() {
                SearchState::Idle
            } else if r.dir_term != t {
                SearchState::Searching
            } else if r.dir_failed {
                SearchState::Failed
            } else {
                SearchState::Idle
            }
        };
        self.as_mut().set_empty_note(QString::from(empty_note(ranked.query.kind, search)));
        self.as_mut().set_search_failed(search == SearchState::Failed);
        self.as_mut().set_retry_pending(retry_pending);
        self.as_mut().set_kind(QString::from(kind.as_str()));
        self.as_mut().set_pick(ranked.pick);
        self.as_mut().set_fallback(ranked.fallback);
        if self.rows_json().to_string() != json {
            self.as_mut().set_rows_json(QString::from(json.as_str()));
        }
        for mxc in want_avatars {
            if !self.as_mut().rust_mut().avatar_pending.insert(mxc.clone()) {
                continue;
            }
            let Some(client) = crate::session::client() else { return };
            let qt = self.qt_thread();
            // (pictures are cached by mxc for every opening; a late one only fills a map)
            crate::bridge::runtime().spawn(async move {
                let uri = matrix_sdk::ruma::OwnedMxcUri::from(mxc.as_str());
                let url = crate::bridge::resolve_avatar(&client, &uri).await.unwrap_or_default();
                let _ = qt.queue(move |mut o| {
                    {
                        let mut r = o.as_mut().rust_mut();
                        r.avatar_pending.remove(&mxc);
                        r.avatars.insert(mxc, url);
                    }
                    o.as_mut().rebuild();
                });
            });
        }
    }
}

/// People you know + who is already in `room_id`, from the local store (no network).
async fn load(client: &matrix_sdk::Client, me: &str, room_id: &str) -> (Vec<KnownPerson>, Vec<String>) {
    use matrix_sdk::RoomMemberships;
    let mut by_id: HashMap<String, KnownPerson> = HashMap::new();
    for room in client.joined_rooms() {
        let dm = room.is_direct().await.unwrap_or(false);
        let Ok(members) = room.members_no_sync(RoomMemberships::JOIN | RoomMemberships::INVITE).await else { continue };
        for m in members {
            let id = m.user_id().to_string();
            if id == me {
                continue;
            }
            let name = m.display_name().unwrap_or("").to_string();
            let avatar = m.avatar_url().map(|u| u.to_string()).unwrap_or_default();
            match by_id.get_mut(&id) {
                None => {
                    by_id.insert(id.clone(), KnownPerson { mxid: id, name, avatar, dm, shared: 1 });
                }
                Some(p) => {
                    p.shared += 1;
                    // a DM's name and picture win (that's how you know them); else the first one seen
                    if (dm && !p.dm) || p.name.is_empty() {
                        if !name.is_empty() {
                            p.name = name;
                        }
                        if !avatar.is_empty() {
                            p.avatar = avatar;
                        }
                    }
                    p.dm |= dm;
                }
            }
        }
    }
    let mut members = Vec::new();
    if let Ok(rid) = matrix_sdk::ruma::RoomId::parse(room_id) {
        if let Some(room) = client.get_room(&rid) {
            if let Ok(ms) = room.members_no_sync(RoomMemberships::JOIN | RoomMemberships::INVITE).await {
                members = ms.into_iter().map(|m| m.user_id().to_string()).collect();
            }
        }
    }
    (by_id.into_values().collect(), members)
}

/// GET /_matrix/client/v3/profile/{id} on YOUR homeserver → (status, errcode, error, displayname, avatar_url).
/// Status 0 = no answer at all. A plain client-API request, so the status and errcode come through as sent (the web's
/// getProfileInfo sees the same).
async fn lookup_profile(client: &matrix_sdk::Client, mxid: &str) -> (u16, String, String, String, String) {
    let base = client.homeserver().to_string();
    let url = format!("{}/_matrix/client/v3/profile/{}", base.trim_end_matches('/'), crate::share_link::encode_component(mxid));
    let Some(token) = client.access_token() else { return (0, String::new(), "not signed in".into(), String::new(), String::new()) };
    let resp = match matrix_sdk::reqwest::Client::new().get(&url).bearer_auth(token).timeout(std::time::Duration::from_secs(20)).send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[people] profile look-up failed to send: {e}");
            return (0, String::new(), String::new(), String::new(), String::new());
        }
    };
    let mut status = resp.status().as_u16();
    let v: Value = resp.text().await.ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    if (200..300).contains(&status) && !v.is_object() {
        status = 0; // a cut-off or unreadable answer is no answer: never "found"
    }
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    (status, s("errcode"), s("error"), s("displayname"), s("avatar_url"))
}
