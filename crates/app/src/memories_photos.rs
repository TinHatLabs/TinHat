//! Memories photos in the desktop app (docs/MEMORIES-PHOTOS.md): every member's
//! `dev.tinhat.memories` list from the room state → the shared rules (`tinhat_widgets::memphotos`, = the web's
//! photos.ts) → each photo's message read the way the timeline reads it (`Room::event`: decrypted with MY keys; the
//! room's history visibility decides what I can fetch) and its image decrypted into the media cache. Nothing goes to the
//! Tin Hat server. Writes go to MY list only (state_key = my user id; Matrix refuses anyone else's key). The web's
//! photosStore.ts does the same.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use matrix_sdk::Room;
use serde_json::{json, Value};
use tinhat_widgets::memphotos as mp;

/// The words of a refused write (the web's MEMORIES_WORDS, the same words).
pub const WORD_FULL: &str = "Your Memories here are full (500 photos). Remove some first.";
pub const WORD_NEWER: &str = "A newer Tin Hat wrote your Memories here. Update Tin Hat to change them.";
pub const WORD_FORBIDDEN: &str = "This room doesn't allow it yet. Ask an admin to open it in Tin Hat.";
pub const WORD_UNREACHABLE: &str = "Couldn't reach the server. Try again.";

/// One fetched photo message: what the rules need + the cached image + the comment count, and when it was read.
#[derive(Clone)]
struct Got {
    fetched: Option<mp::Fetched>,
    content: Value,
    comments: u64,
    media: String,
    /// the message was read but its image couldn't be fetched / decrypted (shown with "Couldn't load", re-read soon)
    media_failed: bool,
    /// 2-8: where its comments go (memphotos::thread_root: a photo posted inside a thread → that thread)
    root: String,
    at: std::time::Instant,
}

/// How long a read stays good: a readable photo 2 minutes (a later redaction, a new comment count and a take-down's
/// effect show at the next open / refresh after that), an unreadable one 30 seconds (a network blip or a key that
/// arrives later never hides a photo for the whole session).
const FRESH_OK: std::time::Duration = std::time::Duration::from_secs(120);
const FRESH_FAIL: std::time::Duration = std::time::Duration::from_secs(30);

fn fresh(g: &Got) -> bool {
    g.at.elapsed() < if g.fetched.is_some() && !g.media_failed { FRESH_OK } else { FRESH_FAIL }
}

/// room \0 event id → what fetching it gave (memory only, this session).
fn cache() -> &'static Mutex<HashMap<String, Got>> {
    static C: OnceLock<Mutex<HashMap<String, Got>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// room → the event ids on MY list (the chat menu's "Add to Memories" / "Remove from Memories" label).
fn mine() -> &'static Mutex<HashMap<String, Vec<String>>> {
    static M: OnceLock<Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

/// Keyed by the session too (captured once per operation): a read made for one account is never handed to the next.
fn key(gen: u64, room: &str, id: &str) -> String {
    format!("{gen}\u{0}{room}\u{0}{id}")
}

/// A local file from a file:// url as QML hands it (percent-escaped: `photo%231.png` is `photo#1.png`), else the
/// plain path form the app's own file_url makes.
pub fn local_path(u: &str) -> std::path::PathBuf {
    if let Ok(parsed) = url::Url::parse(u) {
        if parsed.scheme() == "file" {
            if let Ok(p) = parsed.to_file_path() {
                return p;
            }
        }
    }
    std::path::PathBuf::from(crate::platform::path_from_file_url(u))
}

/// Is this message on MY list here (as last read; a room not read yet answers false and is read in the background)?
pub fn in_my_memories(room_id: &str, event_id: &str) -> bool {
    mine().lock().map(|m| m.get(room_id).is_some_and(|l| l.iter().any(|e| e == event_id))).unwrap_or(false)
}

fn remember_mine(room_id: &str, content: &Value) {
    let ids: Vec<String> = content
        .get("photos")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|p| p.get("event_id").and_then(Value::as_str).map(str::to_string)).collect())
        .unwrap_or_default();
    if let Ok(mut m) = mine().lock() {
        m.insert(room_id.to_string(), ids);
    }
}

/// Read my list into the menu's cache (room open / menu open), my pending write included.
pub async fn refresh_mine(room: &Room) {
    let l = current_for_write(room, crate::session::generation()).await;
    remember_mine(room.room_id().as_str(), &l);
}

/// The people whose take-downs count now: may delete other people's messages (ruma's redact rule over the room's
/// power levels, room-v12 creators included).
pub async fn moderators(room: &Room, people: &[String]) -> Vec<String> {
    let Ok(pl) = room.power_levels().await else { return Vec::new() };
    people
        .iter()
        .filter(|p| matrix_sdk::ruma::UserId::parse(p.as_str()).map(|u| pl.user_can_redact_event_of_other(&u)).unwrap_or(false))
        .cloned()
        .collect()
}

/// The comment count of a photo (the shared rule, memphotos::comment_count, over the event's `unsigned`).
pub fn comment_count(ev: &Value) -> u64 {
    mp::comment_count(ev.get("unsigned").unwrap_or(&Value::Null))
}

async fn fetch_one(room: &Room, id: &str) -> Got {
    let none = Got { fetched: None, content: Value::Null, comments: 0, media: String::new(), media_failed: false, root: String::new(), at: std::time::Instant::now() };
    let Ok(eid) = matrix_sdk::ruma::EventId::parse(id) else { return none };
    let Ok(ev) = room.event(&eid, None).await else { return none };
    let v = ev.raw().deserialize_as::<Value>().unwrap_or(Value::Null);
    let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
    if ty.is_empty() || ty == "m.room.encrypted" {
        return none; // not decryptable here (yet)
    }
    let content = v.get("content").cloned().unwrap_or(Value::Null);
    let fetched = mp::Fetched {
        sender: v.get("sender").and_then(Value::as_str).unwrap_or("").to_string(),
        // Room::event reads within this room (a sync-shaped event carries no room_id)
        room_id: v.get("room_id").and_then(Value::as_str).unwrap_or(room.room_id().as_str()).to_string(),
        kind: ty.to_string(),
        msgtype: content.get("msgtype").and_then(Value::as_str).unwrap_or("").to_string(),
        redacted: v.pointer("/unsigned/redacted_because").is_some(),
        ts: v.get("origin_server_ts").and_then(Value::as_number).cloned(),
    };
    let mut media = String::new();
    let mut media_failed = false;
    if fetched.msgtype == "m.image" && !fetched.redacted {
        let mime = content.pointer("/info/mimetype").and_then(Value::as_str).map(str::to_string);
        media = crate::room::fetch_media(content.clone(), id, mime.as_deref(), crate::room::MediaUse::InlineImage).await.unwrap_or_default();
        media_failed = media.is_empty();
    }
    // 2-8: the count of the thread its comments go to (the photo's own, or the thread it was posted in)
    let root = mp::thread_root(id, &content);
    // (launch-gate ruling) a photo inside a thread shows no count: the thread's count is not the photo's
    let comments = if root == id { comment_count(&v) } else { 0 };
    Got { comments, fetched: Some(fetched), content, media, media_failed, root, at: std::time::Instant::now() }
}

/// The room's Memories photos as I may see them, newest first: `{photos: [{event_id, owner, ts, media, comments,
/// name}], unavailable, iModerate, canAdd, me}` (the Memories window's photosJson).
pub async fn load(room: &Room) -> Value {
    let gen = crate::session::generation();
    let rid = room.room_id().to_string();
    let me = room.own_user_id().to_string();
    let mut all = crate::appstate::get_state_all(room, mp::MEMORIES_TYPE).await;
    // my own list as I last wrote it while sync hasn't brought it back (an add / remove shows at once)
    let mine_now = current_for_write(room, gen).await;
    all.retain(|(k, _, _)| *k != me);
    if !mine_now.is_null() {
        all.push((me.clone(), me.clone(), mine_now));
    }
    let states: Vec<Value> = all.iter().map(|(k, s, c)| json!({"state_key": k, "sender": s, "content": c})).collect();
    if let Some((_, _, c)) = all.iter().find(|(k, s, _)| *k == me && *s == me) {
        remember_mine(&rid, c);
    }
    let mut owners: Vec<String> = all.iter().map(|(k, _, _)| k.clone()).collect();
    owners.push(me.clone());
    owners.sort();
    owners.dedup();
    let mods = moderators(room, &owners).await;
    let cands = mp::candidates(&states, &mods);
    let mut ids: Vec<String> = Vec::new();
    for p in &cands.photos {
        if !cands.hidden.contains(&p.event_id) && !ids.contains(&p.event_id) {
            ids.push(p.event_id.clone());
        }
    }
    // fetch what isn't known yet (newest first, a few at a time: each is one /event read + one decrypt + the image)
    let todo: Vec<String> = {
        let c = cache().lock().map(|c| ids.iter().filter(|i| !c.get(&key(gen, &rid, i)).is_some_and(fresh)).cloned().collect()).unwrap_or_default();
        c
    };
    {
        use futures_util::StreamExt;
        let started = std::time::Instant::now();
        let got: Vec<(String, Got)> = futures_util::stream::iter(todo).map(|id| async move { let g = fetch_one(room, &id).await; (id, g) }).buffer_unordered(4).collect().await;
        // the account changed while these were read: they are not this session's
        if gen == crate::session::generation() {
            let mut untrusted = false;
            if let Ok(mut c) = cache().lock() {
                for (id, g) in got {
                    let k = key(gen, &rid, &id);
                    // 2-3 (review): a redaction arrived while this read was under way: not kept; the room's count moves so
                    // the open window reads again and the server's answer decides
                    if invalidated_since(&k, started) {
                        untrusted = true;
                        continue;
                    }
                    c.insert(k, g);
                }
            }
            if untrusted {
                if let Ok(mut r) = revs().lock() {
                    *r.entry(rid.clone()).or_insert(0) += 1;
                }
            }
        }
    }
    let (fetched, extra): (HashMap<String, Option<mp::Fetched>>, HashMap<String, Got>) = {
        let c = cache().lock().map(|c| c.clone()).unwrap_or_default();
        let mut f = HashMap::new();
        let mut x = HashMap::new();
        for id in &ids {
            if let Some(g) = c.get(&key(gen, &rid, id)) {
                f.insert(id.clone(), g.fetched.clone());
                x.insert(id.clone(), g.clone());
            }
        }
        (f, x)
    };
    let shown = mp::shown(&cands, &fetched, &rid);
    let mut photos = Vec::new();
    for p in &shown.photos {
        let g = extra.get(&p.event_id);
        let name = crate::room::resolve_member_name(room, &p.owner).await;
        photos.push(json!({
            "event_id": p.event_id,
            "owner": p.owner,
            "ts": Value::Number(p.ts.clone()),
            "media": g.map(|g| g.media.clone()).unwrap_or_default(),
            "comments": g.map(|g| g.comments).unwrap_or(0),
            "mediaFailed": g.is_some_and(|g| g.media_failed),
            "threadRoot": g.map(|g| g.root.clone()).filter(|r| !r.is_empty()).unwrap_or_else(|| p.event_id.clone()),
            // 2-8: the Comments button's face (memphotos::comments_face: in a thread → no count, "Comments in thread")
            "commentsFace": mp::comments_face(&p.event_id, &g.map(|g| g.content.clone()).unwrap_or(Value::Null), &json!(g.map(|g| g.comments).unwrap_or(0))),
            "name": name,
        }));
    }
    let can_add = match room.power_levels().await {
        Ok(pl) => {
            use matrix_sdk::ruma::events::StateEventType;
            pl.user_can_send_state(room.own_user_id(), StateEventType::from(mp::MEMORIES_TYPE)) || pl.user_can_send_state(room.own_user_id(), StateEventType::RoomPowerLevels)
        }
        Err(_) => false,
    };
    json!({
        "gen": gen,
        "room": rid,
        "photos": photos,
        "unavailable": shown.unavailable,
        "iModerate": mods.contains(&me),
        "canAdd": can_add,
        "me": me,
        "encrypted": room.encryption_state().is_encrypted(),
    })
}

// ---- 2-3: what the sync changes (the sync loop calls on_sync: add_event_handler doesn't dispatch under sync_once) ----

/// room → how many Memories changes the sync brought (an open Memories window re-reads when its room's count moves).
fn revs() -> &'static Mutex<HashMap<String, u64>> {
    static R: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// key → when a redaction event for it arrived (2-3, review: a read under way then is not trusted; kept 10 minutes).
fn invalidated() -> &'static Mutex<HashMap<String, std::time::Instant>> {
    static I: OnceLock<Mutex<HashMap<String, std::time::Instant>>> = OnceLock::new();
    I.get_or_init(Default::default)
}

fn invalidated_since(k: &str, started: std::time::Instant) -> bool {
    invalidated().lock().map(|m| m.get(k).is_some_and(|at| *at >= started)).unwrap_or(false)
}

/// The room's Memories change count (MemoriesWindow.qml watches it while open).
pub fn rev_of(room_id: &str) -> u64 {
    revs().lock().map(|r| r.get(room_id).copied().unwrap_or(0)).unwrap_or(0)
}

/// What a room's new events do to its Memories: a `dev.tinhat.memories` state event (an add, a remove, a take-down) or
/// the redaction of a photo this device has read (its cached read is dropped: a deleted message never stays on show).
/// When anything changed, the room's count moves; true then.
pub fn note_events<'a>(room_id: &str, events: impl IntoIterator<Item = &'a Value>) -> bool {
    let mut hit = false;
    for v in events {
        let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
        if ty == mp::MEMORIES_TYPE && v.get("state_key").is_some() {
            hit = true;
        } else if ty == "m.room.redaction" {
            // the target: top-level `redacts` (rooms before v11), `content.redacts` (v11+)
            let target = v.get("redacts").and_then(Value::as_str).or_else(|| v.pointer("/content/redacts").and_then(Value::as_str));
            if let Some(t) = target {
                let k = key(crate::session::generation(), room_id, t);
                // the server's next answer decides (a raw redaction event is not proof it took effect: review); a read
                // already under way is not trusted either (load checks invalidated_since)
                if let Ok(mut m) = invalidated().lock() {
                    m.retain(|_, at| at.elapsed() < std::time::Duration::from_secs(600));
                    m.insert(k.clone(), std::time::Instant::now());
                }
                if cache().lock().map(|mut c| c.remove(&k).is_some()).unwrap_or(false) {
                    hit = true;
                }
            }
        }
    }
    if hit {
        if let Ok(mut r) = revs().lock() {
            *r.entry(room_id.to_string()).or_insert(0) += 1;
        }
    }
    hit
}

/// The sync loop's hook (bridge.rs, next to the other on_sync calls): every joined room's new timeline and state events.
pub fn on_sync(response: &matrix_sdk::sync::SyncResponse) {
    for (rid, upd) in &response.rooms.joined {
        let mut evs: Vec<Value> = upd.timeline.events.iter().filter_map(|ev| ev.raw().deserialize_as::<Value>().ok()).collect();
        let state = match &upd.state {
            matrix_sdk::sync::State::Before(v) | matrix_sdk::sync::State::After(v) => v,
        };
        evs.extend(state.iter().filter_map(|raw| raw.deserialize_as::<Value>().ok()));
        if !evs.is_empty() {
            note_events(rid.as_str(), evs.iter());
        }
    }
}

/// Forget what was fetched for one message (it was redacted, or my list changed).
pub fn forget(room_id: &str, event_id: &str) {
    if let Ok(mut c) = cache().lock() {
        c.remove(&key(crate::session::generation(), room_id, event_id));
    }
}

/// What I last wrote per room (session + room → content, its event id, when): the local state store learns my own
/// write only at the next sync, so a second edit right after the first (two photos added in a row) must build on what
/// I wrote, not on the store, or the first one is lost.
struct Written {
    content: Value,
    event_id: String,
    /// the store's event my write replaced (2-5: the write wins only while the store still shows it)
    replaced: String,
}
fn written() -> &'static Mutex<HashMap<String, Written>> {
    static W: OnceLock<Mutex<HashMap<String, Written>>> = OnceLock::new();
    W.get_or_init(Default::default)
}
/// One write of my list per room at a time.
fn write_lock(room_id: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    static L: OnceLock<Mutex<HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    L.get_or_init(Default::default).lock().map(|mut m| m.entry(room_id.to_string()).or_default().clone()).unwrap_or_default()
}
/// My list in the local store: (content, its event id; "" = none).
async fn store_list(room: &Room) -> (Value, String) {
    let me = room.own_user_id().to_string();
    match room.get_state_event(matrix_sdk::ruma::events::StateEventType::from(mp::MEMORIES_TYPE), &me).await {
        Ok(Some(raw)) => {
            let v = match raw {
                matrix_sdk::deserialized_responses::RawAnySyncOrStrippedState::Sync(r) => r.deserialize_as::<Value>().unwrap_or(Value::Null),
                matrix_sdk::deserialized_responses::RawAnySyncOrStrippedState::Stripped(r) => r.deserialize_as::<Value>().unwrap_or(Value::Null),
            };
            (v.get("content").cloned().unwrap_or(Value::Null), v.get("event_id").and_then(Value::as_str).unwrap_or("").to_string())
        }
        _ => (Value::Null, String::new()),
    }
}

/// My list as the app shows it: my last write while sync hasn't brought anything after it (the store still shows the
/// event my write replaced: memphotos::pending_wins), else the store (my write itself, or another device's newer list;
/// 2-5: a stale pending write never hides it). No clocks are compared.
async fn current_for_write(room: &Room, gen: u64) -> Value {
    let wkey = format!("{gen}\u{0}{}", room.room_id());
    let (store, store_id) = store_list(room).await;
    if let Ok(mut w) = written().lock() {
        if let Some(l) = w.get(&wkey) {
            if mp::pending_wins(&store_id, &json!({"event_id": l.event_id, "replaced": l.replaced})) {
                return l.content.clone();
            }
            w.remove(&wkey); // the store has my write, or something newer
        }
    }
    store
}

/// 2-5: my list as the SERVER holds it now (authoritative, under the write lock): Ok(Null) when I have none; Err when
/// the server can't be asked (the write is refused).
async fn server_list(room: &Room) -> Result<Value, String> {
    use matrix_sdk::ruma::api::client::state::get_state_event_for_key::v3::Request;
    let me = room.own_user_id().to_string();
    let req = Request::new(room.room_id().to_owned(), matrix_sdk::ruma::events::StateEventType::from(mp::MEMORIES_TYPE), me);
    match room.client().send(req).await {
        Ok(r) => Ok(serde_json::from_str(r.event_or_content.get()).unwrap_or(Value::Null)),
        // only Matrix's own "no such state" (review: a proxy's 404 is not proof I have no list)
        Err(e) if e.as_client_api_error().is_some_and(|api| matches!(api.error_kind(), Some(matrix_sdk::ruma::api::error::ErrorKind::NotFound))) => Ok(Value::Null),
        Err(e) => Err(e.to_string()),
    }
}

/// Write MY list with `change`; the words of a refusal on error.
async fn write(room: &Room, change: impl FnOnce(&Value) -> mp::Edit) -> Result<(), String> {
    let gen = crate::session::generation();
    let lock = write_lock(room.room_id().as_str());
    let _held = lock.lock().await;
    if gen != crate::session::generation() {
        return Err("Signed out".into()); // queued under another account: never written
    }
    // 2-5: edit what the server holds now (another of my devices may have written since); only when it can't be asked,
    // what this device knows
    // nothing is written when the server can't be asked (review: editing what this device knows could bring back what
    // another device removed)
    let cur = match server_list(room).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[memories] reading my list from the server failed ({e}): not written");
            return Err(WORD_UNREACHABLE.into());
        }
    };
    let replaced = store_list(room).await.1;
    let content = match change(&cur) {
        mp::Edit::Content(m) => Value::Object(m),
        mp::Edit::Error("full") => return Err(WORD_FULL.into()),
        mp::Edit::Error(_) => return Err(WORD_NEWER.into()),
    };
    // an admin's app lowers events["dev.tinhat.memories"] to 0 here (once per room per session; own server's rooms only)
    crate::appstate::ensure_writable(room, &[mp::MEMORIES_TYPE]).await;
    let me = room.own_user_id().to_string();
    match room.send_state_event_raw(mp::MEMORIES_TYPE, &me, content.clone()).await {
        Ok(r) => {
            if gen == crate::session::generation() {
                remember_mine(room.room_id().as_str(), &content);
                let wkey = format!("{gen}\u{0}{}", room.room_id());
                if let Ok(mut w) = written().lock() {
                    w.insert(wkey, Written { content: content.clone(), event_id: r.event_id.to_string(), replaced });
                }
            }
            Ok(())
        }
        Err(e) => {
            let s = e.to_string();
            Err(if s.contains("M_FORBIDDEN") { WORD_FORBIDDEN.to_string() } else { format!("Couldn't save it ({s})") })
        }
    }
}

/// "Add to Memories" on one of MY image messages.
pub async fn add(room: &Room, event_id: &str, ts: f64) -> Result<(), String> {
    let r = json!({"event_id": event_id, "ts": ts as u64});
    write(room, |cur| mp::with_photo(cur, &r)).await
}

/// "Remove from Memories" (my own photo).
pub async fn remove(room: &Room, event_id: &str) -> Result<(), String> {
    write(room, |cur| mp::without_photo(cur, event_id)).await
}

/// A moderator's take-down of someone else's photo.
pub async fn take_down(room: &Room, event_id: &str) -> Result<(), String> {
    write(room, |cur| mp::with_hidden(cur, event_id)).await
}

/// 12.1.17: the files "Add photos" takes (the picker's filter, a paste, a drop, the send itself): every picture type the
/// app sends as an image, SVG excepted: Tin Hat Web's `accept="image/*"` minus image/svg+xml, by the extension's type
/// (room::mime_for_ext). Phone photos (HEIC/HEIF, AVIF) are in: they go through the same cleaner as a chat photo
/// (room::photo_bytes_for_upload), and like there no shipped Qt draws them, so their tile says it can't be shown here.
pub const PHOTO_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "heic", "heif", "hif", "avif"];

/// Is `ext` (any case, without the dot) a file "Add photos" takes? The one rule behind [`PHOTO_EXTS`] and send_photo.
pub fn is_photo_ext(ext: &str) -> bool {
    let mime = crate::room::mime_for_ext(&ext.to_lowercase());
    mime.starts_with("image/") && mime != "image/svg+xml"
}

/// "Add photos": send one image file to the room's chat (encrypted where the room is: the chat's own upload rule,
/// room::attach_upload_checked) and add it to my Memories. → the message's event id.
pub async fn send_photo(room: &Room, path: &std::path::Path) -> Result<String, String> {
    use matrix_sdk::ruma::events::room::MediaSource;
    if room.state() != matrix_sdk::RoomState::Joined {
        return Err("not joined".into());
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let mime_s = crate::room::mime_for_ext(&ext);
    if !is_photo_ext(&ext) {
        return Err("Only photos and screenshots can be added.".into());
    }
    let data = tokio::fs::read(path).await.map_err(|x| format!("read {}: {x}", path.display()))?;
    // "Location is removed from photos you send" (sun-exif): clean the picture before anything is uploaded
    let data = crate::room::photo_bytes_for_upload(mime_s, data).map_err(|e| {
        if e.starts_with("photo location: ") { crate::room::PHOTO_LOCATION_FAIL.to_string() } else { e }
    })?;
    let size = data.len() as u64;
    let (w, h) = crate::clipboard::image_size(path).unwrap_or((0, 0));
    let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("photo").to_string();
    let mime: mime::Mime = mime_s.parse().unwrap_or(mime::IMAGE_PNG);
    let client = room.client();
    let source = crate::room::attach_upload_checked(
        || async { room.latest_encryption_state().await.map_err(|x| x.to_string()) },
        || room.encryption_state(),
        |encrypted| {
            let (client, data, mime) = (&client, &data, &mime);
            async move {
                if encrypted {
                    let mut cur = std::io::Cursor::new(data.as_slice());
                    let f = client.upload_encrypted_file(&mut cur).await.map_err(|x| x.to_string())?;
                    Ok(MediaSource::Encrypted(Box::new(f)))
                } else {
                    let r = client.media().upload(mime, data.clone(), None).await.map_err(|x| x.to_string())?;
                    Ok(MediaSource::Plain(r.content_uri))
                }
            }
        },
    )
    .await?;
    let content = crate::room::attachment_content(&mime, filename, source, w, h, size);
    let mut content = serde_json::to_value(content).map_err(|x| x.to_string())?;
    // 12.1.17: the Memories marker (inside the encryption): quiet at once for 12.1.17+ receivers; the chat hides the
    // photo once my list names it (memphotos::memories_post / hidden_in_chat)
    if let Some(o) = content.as_object_mut() {
        o.insert(mp::POST_KEY.to_string(), json!({"v": 1}));
    }
    let r = room.send_raw("m.room.message", content).await.map_err(|x| x.to_string())?;
    let eid = r.response.event_id.to_string();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0);
    add(room, &eid, now).await?;
    Ok(eid)
}

// ---- 12.1.17: a Memories photo leaves the chat (docs/MEMORIES-PHOTOS.md "In the chat") --------------------------------

/// What the chat hides in this room now: every member's list (mine as I last wrote it, so my own add hides at once) +
/// the moderators now → memphotos::chat_index. The web's memories/chatHide.ts roomChatIndex does the same.
pub async fn chat_index(room: &Room) -> Option<mp::ChatIndex> {
    let gen = crate::session::generation();
    let me = room.own_user_id().to_string();
    let mut all = crate::appstate::get_state_all(room, mp::MEMORIES_TYPE).await;
    let mine_now = current_for_write(room, gen).await;
    all.retain(|(k, _, _)| *k != me);
    if !mine_now.is_null() {
        all.push((me.clone(), me.clone(), mine_now));
    }
    if all.is_empty() {
        return Some(mp::ChatIndex::default());
    }
    let states: Vec<Value> = all.iter().map(|(k, s, c)| json!({"state_key": k, "sender": s, "content": c})).collect();
    let mut people: Vec<String> = all.iter().map(|(k, _, _)| k.clone()).collect();
    people.sort();
    people.dedup();
    let mods = moderators(room, &people).await;
    mp::chat_index(Some(&states), &mods)
}

/// A timeline event (JSON, decrypted where it could be) as the shared chat rules see it.
pub fn chat_msg_of(v: &Value) -> Option<mp::ChatMsg<'_>> {
    let content = v.get("content")?;
    if !content.is_object() {
        return None;
    }
    Some(mp::ChatMsg {
        event_id: v.get("event_id")?.as_str()?,
        sender: v.get("sender")?.as_str()?,
        kind: v.get("type")?.as_str()?,
        redacted: v.pointer("/unsigned/redacted_because").is_some(),
        msgtype: content.get("msgtype").and_then(Value::as_str).unwrap_or(""),
        marker: content.get(mp::POST_KEY),
        in_thread: !tinhat_widgets::threads::thread_of(content).is_empty(),
    })
}

/// Round 2 (shared vectors `counts`): (silenced, uncounted) for a timeline / notification event (JSON, decrypted).
pub fn counts_event(index: Option<&mp::ChatIndex>, v: &Value) -> (bool, bool) {
    mp::chat_counts(index, chat_msg_of(v).as_ref())
}

/// Is this event hidden from the chat by `index`?
pub fn hidden_event(index: Option<&mp::ChatIndex>, v: &Value) -> bool {
    mp::hidden_in_chat(index, chat_msg_of(v).as_ref())
}

/// A photo posted by "Add photos" (its marker): no notification, no receive sound, not counted unread.
pub fn post_event(v: &Value) -> bool {
    mp::memories_post(chat_msg_of(v).as_ref())
}

/// room → the photos the chat hid at its last load (a jump to one opens it in Memories instead of hunting the history).
fn hidden_ids() -> &'static Mutex<HashMap<String, std::collections::HashSet<String>>> {
    static H: OnceLock<Mutex<HashMap<String, std::collections::HashSet<String>>>> = OnceLock::new();
    H.get_or_init(Default::default)
}

/// The room's load found these hidden photos (the main view's load replaces the set; a thread panel's adds to it).
pub fn note_hidden(room_id: &str, ids: impl IntoIterator<Item = String>, replace: bool) {
    if let Ok(mut m) = hidden_ids().lock() {
        let e = m.entry(room_id.to_string()).or_default();
        if replace {
            e.clear();
        }
        e.extend(ids);
    }
}

/// Did the chat hide this message at the room's last load?
pub fn hidden_in_chat(room_id: &str, event_id: &str) -> bool {
    hidden_ids().lock().map(|m| m.get(room_id).is_some_and(|s| s.contains(event_id))).unwrap_or(false)
}

/// Forget every room's hidden set (sign-out).
pub fn forget_hidden() {
    if let Ok(mut m) = hidden_ids().lock() {
        m.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 12.1.17: Memories takes phone photos (.heic/.heif/.avif) like Tin Hat Web (`image/*` minus SVG), and every way in
    /// (the picker's filter, a paste, a drop, the gate hook: MemoriesWindow.qml addPhotoUrls) asks this one list.
    #[test]
    fn add_photos_takes_phone_photos_like_the_web() {
        for e in ["heic", "HEIC", "heif", "hif", "avif", "png", "jpg", "jpeg", "gif", "webp", "bmp"] {
            assert!(is_photo_ext(e), "{e} is a photo");
        }
        for e in ["svg", "txt", "mp4", "pdf", "", "heic.txt"] {
            assert!(!is_photo_ext(e), "{e} is not");
        }
        // the list is exactly the rule (nothing listed that the send refuses, every phone format listed)
        for e in PHOTO_EXTS {
            assert!(is_photo_ext(e), "{e} listed but refused");
        }
        for e in ["heic", "heif", "hif", "avif"] {
            assert!(PHOTO_EXTS.contains(&e), "{e} missing from the list");
        }
        // and the other way (review round): every extension the send maps to a picture (SVG aside) is listed
        let room = include_str!("room.rs");
        let m = &room[room.find("pub(crate) fn mime_for_ext(ext: &str)").expect("mime_for_ext")..];
        let m = &m[..m.find("\n}\n").unwrap()];
        let mut images = 0;
        for line in m.lines().filter(|l| l.contains("=> \"image/") && !l.contains("image/svg+xml")) {
            for ext in line[..line.find("=>").unwrap()].split('|').map(|x| x.trim().trim_matches('"')) {
                assert!(PHOTO_EXTS.contains(&ext), "{ext} is sent as a picture but Add photos doesn't take it");
                images += 1;
            }
        }
        assert_eq!(images, PHOTO_EXTS.len(), "the list is exactly the picture types");
        let q = include_str!("../qml/MemoriesWindow.qml");
        let add = &q[q.find("function addPhotoUrls(urls)").expect("addPhotoUrls")..];
        let add = &add[..add.find("\n    }\n").unwrap()];
        assert!(add.contains("mem.photoExts.indexOf("), "addPhotoUrls asks the list: {add}");
        assert!(!add.contains("(png|jpe?g|gif|webp|bmp)"), "no second, shorter list");
        let picker = &q[q.find("id: addPhotosDialog").unwrap()..];
        let picker = &picker[..picker.find("onAccepted").unwrap()];
        assert!(picker.contains("nameFilters: [\"Images (\" + mem.photoExts.map("), "the picker's filter is the list: {picker}");
        assert!(q.contains("readonly property var photoExts: mem.vodctl ? JSON.parse(String(mem.vodctl.memoryPhotoExts() || \"[]\")) : []"));
    }

    #[test]
    fn comment_count_reads_the_thread_aggregation() {
        assert_eq!(comment_count(&json!({"unsigned": {"m.relations": {"m.thread": {"count": 3}}}})), 3);
        assert_eq!(comment_count(&json!({"unsigned": {}})), 0);
        assert_eq!(comment_count(&json!({"unsigned": {"m.relations": {"m.thread": {"count": "3"}}}})), 0);
    }

    #[test]
    fn a_redaction_or_a_list_change_moves_the_rooms_count_and_drops_the_photo_2_3() {
        let room = "!fx23:x";
        let k = key(crate::session::generation(), room, "$p");
        cache().lock().unwrap().insert(k.clone(), Got { fetched: None, content: Value::Null, comments: 0, media: "file:///p.png".into(), media_failed: false, root: "$p".into(), at: std::time::Instant::now() });
        let r0 = rev_of(room);
        // a redaction of a photo this device has read (v11 form: content.redacts)
        assert!(note_events(room, [&json!({"type": "m.room.redaction", "redacts": null, "content": {"redacts": "$p"}})]));
        assert!(!cache().lock().unwrap().contains_key(&k), "the redacted photo's cached read is gone");
        assert_eq!(rev_of(room), r0 + 1);
        // an add / remove / take-down (someone's list)
        assert!(note_events(room, [&json!({"type": mp::MEMORIES_TYPE, "state_key": "@a:x", "content": {"v": 1, "photos": []}})]));
        assert_eq!(rev_of(room), r0 + 2);
        // anything else changes nothing
        let before = std::time::Instant::now();
        assert!(!note_events(room, [&json!({"type": "m.room.message", "content": {"body": "hi"}}), &json!({"type": "m.room.redaction", "redacts": "$unknown"})]));
        assert_eq!(rev_of(room), r0 + 2);
        // (review) a read of "$unknown" under way when its redaction arrived is not trusted
        assert!(invalidated_since(&key(crate::session::generation(), room, "$unknown"), before));
    }

    #[test]
    fn a_timeline_event_as_the_chat_rules_see_it_12_1_17() {
        let states = vec![json!({"state_key": "@a:x", "sender": "@a:x", "content": {"v": 1, "photos": [{"event_id": "$p", "ts": 1}]}})];
        let idx = mp::chat_index(Some(&states), &[]);
        let img = |id: &str, sender: &str| json!({"event_id": id, "sender": sender, "type": "m.room.message", "content": {"msgtype": "m.image", "body": "a.png"}});
        assert!(hidden_event(idx.as_ref(), &img("$p", "@a:x")));
        // someone else's message with that id, another photo, no index (fail closed)
        assert!(!hidden_event(idx.as_ref(), &img("$p", "@b:x")));
        assert!(!hidden_event(idx.as_ref(), &img("$q", "@a:x")));
        assert!(!hidden_event(None, &img("$p", "@a:x")));
        // redacted (the window's own mark), still encrypted, no content
        let mut red = img("$p", "@a:x");
        red["unsigned"] = json!({"redacted_because": {"type": "m.room.redaction"}});
        assert!(!hidden_event(idx.as_ref(), &red));
        assert!(!hidden_event(idx.as_ref(), &json!({"event_id": "$p", "sender": "@a:x", "type": "m.room.encrypted", "content": {"algorithm": "m.megolm.v1.aes-sha2"}})));
        assert!(!hidden_event(idx.as_ref(), &json!({"event_id": "$p", "sender": "@a:x", "type": "m.room.message"})));
        // the "Add photos" marker: quiet, never hidden by itself
        let mut post = img("$n", "@a:x");
        post["content"][mp::POST_KEY] = json!({"v": 1});
        assert!(post_event(&post));
        assert!(!hidden_event(idx.as_ref(), &post));
        assert!(!post_event(&img("$n", "@a:x")));
    }

    #[test]
    fn the_windows_words_are_the_shared_words_12_1_17() {
        let qml = include_str!("../qml/MemoriesWindow.qml");
        for w in [mp::ADD_TIP, mp::REMOVE_TEXT, mp::DELETE_TITLE, mp::DELETE_TEXT] {
            assert!(qml.contains(&format!("\"{w}\"")), "MemoriesWindow.qml lacks {w:?}");
        }
        // the photo isn't in the chat any more: no "Show in chat", no "also posted in the chat"
        assert!(!qml.contains("text: \"Show in chat\""));
        assert!(!qml.contains("also posted in the chat"));
        assert!(!qml.contains("The photo stays in the chat."));
    }

    #[test]
    fn the_menu_cache_follows_my_list() {
        remember_mine("!r:x", &json!({"v": 1, "photos": [{"event_id": "$a", "ts": 1}]}));
        assert!(in_my_memories("!r:x", "$a"));
        assert!(!in_my_memories("!r:x", "$b"));
        assert!(!in_my_memories("!other:x", "$a"));
        remember_mine("!r:x", &json!({}));
        assert!(!in_my_memories("!r:x", "$a"));
    }
}
