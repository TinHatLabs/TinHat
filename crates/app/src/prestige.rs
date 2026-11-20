//! PrestigeController — the prestige rank store of the app (GAMES-V1-SPEC rev 2 §1.1 / §3.3 / §3.4): prestige_cache.rs
//! wired to the Matrix client, each space owner's well-known and its play API. Tin Hat Web's
//! `src/features/prestige/store.ts` is the same wiring.
//!
//!   QML  Prestige.want(mxid, contextId)   a name is on screen (PrestigeToken.qml asks). contextId = the room the
//!                                         surface is about, or a space's own id on its own surfaces; "" = no token
//!        Prestige.prestigeOf(mxid, contextId) / lookOf(…)  read `viewJson` (AppCore mirrors it into the singleton)
//!   Rust apply_push(owner, frame)         the play client hands every {"t":"rank"} frame of an owner's socket here
//!        share_session(owner, s, exp)     the play client shares its play session (one OpenID exchange per server)
//!
//! Whose rank (§3.3): play_rules::rank_context — a room → its game space (play_rules::game_space, app mode, on the
//! state this client holds), a space room → itself, nothing else (no token outside a space). The owner = the server of
//! the space's create-event sender, never a via; its API comes from ITS well-known (`features.play === true` or no
//! tokens). A surface's space is cached 5 min and dropped on any m.space.parent / m.space.child / own membership
//! change. The play session is the play client's when it shared one, else this engine's own OpenID exchange (§6.1).
//! The cache forgets everything when the signed-in account changes.
//!
//! viewJson = {"ctx": {contextId: "owner|space" | ""}, "scopes": {"owner|space": {mxid: {"r": rank, "g": glyph,
//! "c": "muted"|"text"|"accent", "s": shine}}}} — only tokens drawn now (rank ≥ 1, ≤ 1 h old, games on there).
//!
//! Lab fixture (until the server's contract freeze, and for screenshots): `TINHAT_GUI_PRESTIGE_FIXTURE` =
//! {"ranks": {"<space id>"|"*": {"<mxid>": rank}}, "ctx"?: {"<context id>"|"*": "<space id>"}} answers instead of the
//! network (or, instead of "ranks", "play": {"<owner>"|"*": "<api>/play/v1"} runs the real session exchange and ranks
//! reads against that API, e.g. the contract mock), through the same cache (every owner then counts as having games); `ctx` maps a surface to a space without
//! any space state (screenshots on rooms that aren't in a space). The same shape as the web's debug-hook fixture.
//! Compiled only with `cfg(tinhat_env_selftests)`: a release build has no such switch.
//! Logs carry counts only — never anybody's id or rank.

use core::pin::Pin;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use cxx_qt::{CxxQtThread, Threading};
use cxx_qt_lib::QString;
use serde_json::{json, Value};

use crate::play_rules::{game_space_of, prestige_tier, rank_context, server_of_user_id, PrestigeLook};
use crate::prestige_cache::{Outcome, RankCache, Scope, Step};

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
/// The view is rebuilt at least this often (a rank older than 1 h must disappear by itself).
const TICK_MS: u64 = 60_000;
/// A surface's space is re-resolved after this long (and at once on a space-link / own membership change).
const SPACE_TTL_MS: u64 = 5 * 60_000;

#[cxx_qt::bridge]
pub mod qobject {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name] // snake_case Rust -> camelCase QML
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, view_json)] // QML: viewJson — see the module doc
        #[qproperty(i32, rev)] //          QML: rev — bumps with every viewJson change
        type PrestigeController = super::PrestigeControllerRust;

        /// QML: start() — attach this controller to the engine (once per process; never in the recorder).
        #[qinvokable]
        fn start(self: Pin<&mut PrestigeController>);
        /// QML: want(mxid, contextId) — this name is on screen (contextId = a room or a space's own id; "" = no token).
        #[qinvokable]
        fn want(self: Pin<&mut PrestigeController>, mxid: &QString, context_id: &QString);
        /// QML: forget(mxid, contextId) — the label that asked is gone (refcounted with want).
        #[qinvokable]
        fn forget(self: Pin<&mut PrestigeController>, mxid: &QString, context_id: &QString);
        /// QML: push(owner, frameJson) — a play-socket frame (the QML side of a play client, if any).
        #[qinvokable]
        fn push(self: Pin<&mut PrestigeController>, server: &QString, frame_json: &QString) -> bool;
    }

    impl cxx_qt::Threading for PrestigeController {}
}

#[derive(Default)]
pub struct PrestigeControllerRust {
    view_json: QString,
    rev: i32,
}

/// The glyph for a look (= the web's PrestigeToken.tsx glyphFor).
pub fn glyph_for(look: &PrestigeLook) -> &'static str {
    if look.star {
        "prestige-star-symbolic"
    } else if look.ring {
        "prestige-ring-symbolic"
    } else if look.glyph == "filled" {
        "prestige-filled-symbolic"
    } else {
        "prestige-symbolic"
    }
}

// ───────────────────────────────────── the engine ─────────────────────────────────────

#[derive(Default)]
struct Engine {
    cache: RankCache,
    /// the account the cache belongs to ("" = none yet)
    me: String,
    /// surface context (room / space id) → its scope (None = no token) and when it was resolved
    ctx: HashMap<String, (Option<Scope>, u64)>,
    /// contexts being resolved → the names that asked meanwhile (all wanted again once it is known)
    resolving: HashMap<String, std::collections::BTreeSet<String>>,
    /// bumps on every space-link / own membership change (a resolve that started before one is not kept)
    space_gen: u64,
    /// bumps on every account change: a completion of an older account's request is dropped
    acct_gen: u64,
    /// what the labels on screen show: context → mxid → how many labels (want/forget)
    interest: HashMap<String, BTreeMap<String, u32>>,
    /// play sessions per server: (session, expires ms)
    sessions: HashMap<String, (String, u64)>,
    /// what the play client shared
    shared: HashMap<String, (String, u64)>,
    started: bool,
    last_view: String,
}

fn engine() -> &'static Mutex<Engine> {
    static E: OnceLock<Mutex<Engine>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(Engine::default()))
}

fn lock() -> std::sync::MutexGuard<'static, Engine> {
    engine().lock().unwrap_or_else(|p| p.into_inner())
}

fn sink() -> &'static Mutex<Option<CxxQtThread<qobject::PrestigeController>>> {
    static S: OnceLock<Mutex<Option<CxxQtThread<qobject::PrestigeController>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

fn wake() -> &'static tokio::sync::Notify {
    static W: OnceLock<tokio::sync::Notify> = OnceLock::new();
    W.get_or_init(tokio::sync::Notify::new)
}

fn spawn<F: std::future::Future<Output = ()> + Send + 'static>(f: F) {
    crate::bridge::runtime().spawn(f);
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// The signed-in account (""), and the cache reset when it changed.
fn current_me(e: &mut Engine) -> String {
    let me = crate::session::client().and_then(|c| c.user_id().map(|u| u.to_string())).unwrap_or_default();
    if me != e.me {
        e.cache.clear();
        e.ctx.clear();
        e.resolving.clear();
        e.sessions.clear();
        e.shared.clear();
        e.interest.clear();
        e.acct_gen += 1;
        e.me = me.clone();
        if let Some(c) = crate::session::client().filter(|_| !me.is_empty()) {
            watch_spaces(&c);
        }
    }
    me
}

/// Any m.space.parent / m.space.child event, or a change of my own membership: every surface's space is resolved again
/// (the labels on screen keep their tokens: each context they show is re-resolved and its names re-held).
fn drop_spaces() {
    let ctxs: Vec<String> = {
        let mut e = lock();
        e.space_gen += 1;
        let ctxs: Vec<String> = e.interest.keys().cloned().collect();
        for c in &ctxs {
            if let Some((Some(old), _)) = e.ctx.get(c).cloned() {
                let names: Vec<String> = e.interest.get(c).map(|m| m.keys().cloned().collect()).unwrap_or_default();
                for n in names {
                    e.cache.release(&old, &n);
                }
            }
        }
        e.ctx.clear();
        ctxs
    };
    for c in ctxs {
        resolve_soon(&c);
    }
    publish();
}

/// Start resolving a context (once at a time); its interested names are wanted again when it lands.
fn resolve_soon(ctx_id: &str) {
    let mut e = lock();
    if e.resolving.contains_key(ctx_id) {
        return;
    }
    e.resolving.insert(ctx_id.to_string(), Default::default());
    let (gen, acct) = (e.space_gen, e.acct_gen);
    let id = ctx_id.to_string();
    drop(e);
    spawn(async move { resolve_ctx(id, gen, acct).await });
}

fn watch_spaces(c: &matrix_sdk::Client) {
    use matrix_sdk::ruma::events::room::member::SyncRoomMemberEvent;
    use matrix_sdk::ruma::events::space::{child::SyncSpaceChildEvent, parent::SyncSpaceParentEvent};
    c.add_event_handler(|_ev: SyncSpaceParentEvent| async { drop_spaces() });
    c.add_event_handler(|_ev: SyncSpaceChildEvent| async { drop_spaces() });
    c.add_event_handler(|ev: SyncRoomMemberEvent, room: matrix_sdk::Room, client: matrix_sdk::Client| async move {
        if client.user_id().is_some_and(|me| me.as_str() == ev.state_key().as_str()) {
            drop_spaces();
        } else if room.is_space() && *ev.membership() != matrix_sdk::ruma::events::room::member::MembershipState::Join {
            // someone else left (or was removed from) a space: no rank shown for them there any more (§1.1)
            lock().cache.forget_person(room.room_id().as_str(), ev.state_key().as_str(), now_ms());
            publish();
        }
    });
}

fn view_of(e: &Engine, now: u64) -> String {
    let mut scopes = serde_json::Map::new();
    for (key, m) in e.cache.view(now) {
        let mut o = serde_json::Map::new();
        for (id, rank) in m {
            let look = prestige_tier(rank);
            if look.tier == 0 {
                continue;
            }
            o.insert(id, json!({"r": rank, "g": glyph_for(&look), "c": look.colour, "s": look.shine}));
        }
        scopes.insert(key, Value::Object(o));
    }
    let ctx: BTreeMap<&String, String> = e.ctx.iter().map(|(k, (sc, _))| (k, sc.as_ref().map(Scope::key).unwrap_or_default())).collect();
    json!({"ctx": ctx, "scopes": scopes}).to_string()
}

fn publish() {
    let Some(qt) = sink().lock().ok().and_then(|s| s.clone()) else { return };
    let view = {
        let mut e = lock();
        let v = view_of(&e, now_ms());
        if v == e.last_view {
            return;
        }
        e.last_view = v.clone();
        v
    };
    let _ = qt.queue(move |mut o| {
        o.as_mut().set_view_json(QString::from(&view));
        let r = *o.rev() + 1;
        o.as_mut().set_rev(r);
    });
}

/// The play client: every {"t":"rank"} frame of `owner`'s play socket (it can only change that owner's spaces).
pub fn apply_push(owner: &str, frame: &Value) -> bool {
    let ok = lock().cache.push(owner, frame, now_ms());
    if ok {
        publish();
    }
    ok
}

/// The play client shares its session for a server (so the ranks read needs no exchange of its own).
pub fn share_session(server: &str, session: &str, expires_ms: u64) {
    lock().shared.insert(server.to_string(), (session.to_string(), expires_ms));
}

fn want_inner(mxid: &str, ctx_id: &str) {
    if ctx_id.is_empty() {
        return; // no context: no token (rev 2 §3.3)
    }
    let now = now_ms();
    let mut e = lock();
    if current_me(&mut e).is_empty() {
        return;
    }
    *e.interest.entry(ctx_id.to_string()).or_default().entry(mxid.to_string()).or_insert(0) += 1;
    let scope = match e.ctx.get(ctx_id) {
        Some((sc, at)) if now.saturating_sub(*at) < SPACE_TTL_MS => sc.clone(),
        _ => {
            drop(e);
            resolve_soon(ctx_id);
            return;
        }
    };
    let Some(scope) = scope else { return };
    e.cache.hold(&scope, mxid);
    e.cache.get(Some(&scope), mxid, now);
    drop(e);
    wake().notify_one();
}

fn forget_inner(mxid: &str, ctx_id: &str) {
    if ctx_id.is_empty() {
        return;
    }
    let mut e = lock();
    let gone = match e.interest.get_mut(ctx_id).and_then(|m| m.get_mut(mxid)) {
        Some(n) => {
            *n = n.saturating_sub(1);
            *n == 0
        }
        None => false,
    };
    if gone {
        if let Some(m) = e.interest.get_mut(ctx_id) {
            m.remove(mxid);
            if m.is_empty() {
                e.interest.remove(ctx_id);
            }
        }
    }
    if gone {
        if let Some((Some(sc), _)) = e.ctx.get(ctx_id).cloned() {
            // another surface of the same space may still show this name (two rooms of one space): keep it held then
            let still = e.interest.iter().any(|(c, m)| m.contains_key(mxid) && matches!(e.ctx.get(c), Some((Some(o), _)) if *o == sc));
            if !still {
                e.cache.release(&sc, mxid);
            }
        }
    }
}

/// A room's or space's state as game_space (app mode) reads it; None when this client isn't joined to it.
async fn held_space(client: &matrix_sdk::Client, id: &str) -> Option<Value> {
    use matrix_sdk::ruma::events::StateEventType;
    let rid = matrix_sdk::ruma::RoomId::parse(id).ok()?;
    let room = client.get_room(&rid)?;
    if room.state() != matrix_sdk::RoomState::Joined {
        return None;
    }
    let create = crate::foreign_room::create_event(&room).await;
    let child_events = room.get_state_events(StateEventType::SpaceChild).await;
    // a joined space whose state hasn't arrived (no create event) or can't be read is unknown, not invalid:
    // game_space stops there (`unavailable`) instead of falling through to another parent
    let partial = create.is_none() || child_events.is_err();
    let children: Vec<Value> = child_events
        .ok()
        .unwrap_or_default()
        .iter()
        .filter_map(crate::appstate::raw_to_value)
        .map(|ev| json!({"room": ev.get("state_key").cloned().unwrap_or(Value::Null), "content": ev.get("content").cloned().unwrap_or(Value::Null)}))
        .collect();
    Some(json!({
        "create_sender": create.as_ref().and_then(|c| c.get("sender")).cloned().unwrap_or(Value::Null),
        "create_type": create.as_ref().and_then(|c| c.get("content")).and_then(|c| c.get("type")).cloned().unwrap_or(Value::Null),
        "partial": partial,
        "children": children,
    }))
}

/// The surface's scope (§1.1 / §3.3): a space room → itself; a room → its game space; Err(()) = not in the store yet.
async fn scope_of(ctx_id: &str) -> Result<Option<Scope>, ()> {
    use matrix_sdk::ruma::events::StateEventType;
    if let Some(map) = fixture().and_then(|f| f.get("ctx").cloned()) {
        let space = map.get(ctx_id).or_else(|| map.get("*")).and_then(Value::as_str).filter(|s| s.starts_with('!'));
        return Ok(space.map(|s| Scope { owner: "fixture.invalid".into(), space: s.to_string() }));
    }
    let client = crate::session::client().ok_or(())?;
    let rid = matrix_sdk::ruma::RoomId::parse(ctx_id).map_err(|_| ())?;
    let room = client.get_room(&rid).ok_or(())?;
    let create = crate::foreign_room::create_event(&room).await;
    let create_type = create.as_ref().and_then(|c| c.get("content")).and_then(|c| c.get("type")).cloned().unwrap_or(Value::Null);
    let (surface, owner) = if create_type.as_str() == Some("m.space") {
        if room.state() != matrix_sdk::RoomState::Joined {
            return Ok(None);
        }
        let owner = create.as_ref().and_then(|c| c.get("sender")).and_then(Value::as_str).and_then(server_of_user_id).map(str::to_string);
        (json!({"kind": "space", "space": ctx_id}), owner)
    } else {
        let parents: Vec<Value> = room
            .get_state_events(StateEventType::SpaceParent)
            .await
            .ok()
            .unwrap_or_default()
            .iter()
            .filter_map(crate::appstate::raw_to_value)
            .map(|ev| json!({"space": ev.get("state_key").cloned().unwrap_or(Value::Null), "content": ev.get("content").cloned().unwrap_or(Value::Null)}))
            .collect();
        let mut spaces = serde_json::Map::new();
        for p in &parents {
            if let Some(id) = p.get("space").and_then(Value::as_str) {
                if let Some(h) = held_space(&client, id).await {
                    spaces.insert(id.to_string(), h);
                }
            }
        }
        let g = game_space_of(&json!({"mode": "app", "room": {"id": ctx_id, "create_type": create_type, "parents": parents}, "spaces": spaces}));
        let gs = if g.why.is_empty() { g.space.clone() } else { None };
        (json!({"kind": "room", "room": ctx_id, "game_space": gs}), g.server)
    };
    Ok(match (rank_context(&surface), owner) {
        (Some(space), Some(owner)) => Some(Scope { owner, space }),
        _ => None,
    })
}

async fn resolve_ctx(ctx_id: String, gen: u64, acct: u64) {
    let res = scope_of(&ctx_id).await;
    let again = {
        let mut e = lock();
        e.resolving.remove(&ctx_id);
        if e.acct_gen != acct {
            return; // another account by now: nothing of this one is kept
        }
        match res {
            // a space link changed while resolving: resolve again with the new state
            Ok(_) if e.space_gen != gen => true,
            Ok(sc) => {
                let now = now_ms();
                // a re-resolution that changes the context's space releases what it held in the old one (unless another
                // shown context of that old space still shows the name)
                if let Some((Some(old), _)) = e.ctx.get(&ctx_id).cloned() {
                    if sc.as_ref() != Some(&old) {
                        let names: Vec<String> = e.interest.get(&ctx_id).map(|m| m.keys().cloned().collect()).unwrap_or_default();
                        for n in names {
                            let still = e.interest.iter().any(|(c, m)| {
                                c != &ctx_id && m.contains_key(&n) && matches!(e.ctx.get(c), Some((Some(o), _)) if *o == old)
                            });
                            if !still {
                                e.cache.release(&old, &n);
                            }
                        }
                    }
                }
                e.ctx.insert(ctx_id.clone(), (sc.clone(), now));
                if let Some(sc) = sc {
                    let names: Vec<String> = e.interest.get(&ctx_id).map(|m| m.keys().cloned().collect()).unwrap_or_default();
                    for mxid in names {
                        e.cache.hold(&sc, &mxid);
                        e.cache.get(Some(&sc), &mxid, now);
                    }
                }
                false
            }
            Err(()) => {
                // not in the store yet: no token for now, asked again in 30 s (the loop re-resolves expired contexts)
                let retry_at = now_ms().saturating_sub(SPACE_TTL_MS - 30_000);
                e.ctx.insert(ctx_id.clone(), (None, retry_at));
                return;
            }
        }
    };
    if again {
        resolve_soon(&ctx_id);
        return;
    }
    wake().notify_one();
    publish();
}

// ---- I/O ------------------------------------------------------------------------------------------------------------

#[cfg(tinhat_env_selftests)]
fn fixture() -> Option<Value> {
    let raw = std::env::var("TINHAT_GUI_PRESTIGE_FIXTURE").ok()?;
    serde_json::from_str::<Value>(&raw).ok().filter(Value::is_object)
}

#[cfg(not(tinhat_env_selftests))]
fn fixture() -> Option<Value> {
    None
}

/// `dev.tinhat.server` of a well-known answer → `<api>/play/v1` when features.play === true, else None.
pub fn play_base_from_well_known(wk: &Value) -> Option<String> {
    let d = wk.get("dev.tinhat.server")?;
    if d.get("features")?.get("play")? != &Value::Bool(true) {
        return None;
    }
    let api = d.get("api")?.as_str()?;
    let rest = api.strip_prefix("https://").or_else(|| api.strip_prefix("http://"))?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() || host.chars().any(char::is_whitespace) {
        return None;
    }
    Some(format!("{}/play/v1", api.trim_end_matches('/')))
}

async fn fetch_base(server: &str) -> Option<String> {
    if let Some(fx) = fixture() {
        // "play": a real play API per owner (e.g. the contract mock): the real session + ranks reads go there
        if let Some(play) = fx.get("play").and_then(Value::as_object) {
            return play.get(server).or_else(|| play.get("*")).and_then(Value::as_str).map(str::to_string);
        }
        return Some(format!("fixture://{server}"));
    }
    let url = format!("https://{server}/.well-known/matrix/client");
    let resp = crate::proxy::http(Some(HTTP_TIMEOUT)).get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let mut v: Value = serde_json::from_str(&resp.text().await.ok()?).ok()?;
    crate::play_net::well_known_seen(&mut v);
    play_base_from_well_known(&v)
}

async fn own_session(server: &str, base: &str, renew: bool) -> Option<String> {
    let now = now_ms();
    {
        let mut e = lock();
        if !renew {
            if let Some((s, exp)) = e.shared.get(server).filter(|(_, exp)| *exp > now + 60_000) {
                let _ = exp;
                return Some(s.clone());
            }
            if let Some((s, _)) = e.sessions.get(server).filter(|(_, exp)| *exp > now + 60_000) {
                return Some(s.clone());
            }
        }
        e.sessions.remove(server);
        if renew {
            e.shared.remove(server);
        }
    }
    let client = crate::session::client()?;
    let me = client.user_id()?.to_string();
    let openid = match client.account().request_openid_token().await {
        Ok(t) => t,
        Err(_) => {
            eprintln!("[prestige] play session: the OpenID request failed");
            return None;
        }
    };
    let body = json!({
        "openid": {
            "access_token": openid.access_token,
            "token_type": openid.token_type,
            "matrix_server_name": openid.matrix_server_name,
            "expires_in": openid.expires_in.as_secs(),
        },
        "user_id": me,
        "client": {"kind": "desktop", "v": env!("CARGO_PKG_VERSION"), "pad": false},
    });
    crate::play_net::note_request("POST", "/session (prestige)");
    let resp = crate::proxy::http(Some(HTTP_TIMEOUT))
        .post(format!("{base}/session"))
        .header("Content-Type", "application/json")
        .body(serde_json::to_vec(&body).ok()?)
        .send()
        .await;
    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[prestige] play session: no answer ({})", if e.is_timeout() { "timeout" } else if e.is_connect() { "connect" } else { "request" });
            return None;
        }
    };
    if !resp.status().is_success() {
        eprintln!("[prestige] play session: HTTP {}", resp.status().as_u16());
        return None;
    }
    let v: Value = serde_json::from_str(&resp.text().await.ok()?).ok()?;
    let session = v.get("session").and_then(Value::as_str)?.to_string();
    if v.get("v").and_then(Value::as_f64) != Some(1.0) || session.len() < 16 || v.get("user").and_then(Value::as_str) != Some(me.as_str()) {
        return None;
    }
    let exp = v.get("expires").and_then(Value::as_u64).unwrap_or(now + 60 * 60_000);
    {
        let mut e = lock();
        if e.me != me {
            return None; // the account changed during the exchange: this session belongs to nobody here
        }
        e.sessions.insert(server.to_string(), (session.clone(), exp));
    }
    Some(session)
}

async fn get_ranks(base: &str, session: &str, space: &str, ids: &[String]) -> (u16, Value) {
    let mut url = format!("{base}/ranks?space={}&", urlencode(space));
    let q: Vec<String> = ids.iter().map(|id| format!("u={}", urlencode(id))).collect();
    url.push_str(&q.join("&"));
    crate::play_net::note_request("GET", "/ranks (prestige)");
    let r = crate::proxy::http(Some(HTTP_TIMEOUT)).get(&url).header("Authorization", format!("Play {session}")).send().await;
    match r {
        Ok(resp) => {
            let st = resp.status().as_u16();
            let v = resp.text().await.ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or(Value::Null);
            (st, v)
        }
        Err(_) => (0, Value::Null),
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The read and when it actually went out (after any session exchange: the 2 s gap counts from there).
async fn read_ranks(scope: &Scope, base: &str, ids: &[String], renew: bool) -> (Outcome, u64) {
    read_ranks_inner(scope, base, ids, renew).await
}

async fn read_ranks_inner(scope: &Scope, base: &str, ids: &[String], renew: bool) -> (Outcome, u64) {
    let (server, space) = (scope.owner.as_str(), scope.space.as_str());
    if let Some(fx) = fixture().filter(|_| base.starts_with("fixture://")).and_then(|f| f.get("ranks").cloned()) {
        let mut table = serde_json::Map::new();
        for key in ["*", space] {
            if let Some(o) = fx.get(key).and_then(Value::as_object) {
                for (k, v) in o {
                    table.insert(k.clone(), v.clone());
                }
            }
        }
        let mut ranks = serde_json::Map::new();
        for id in ids {
            if let Some(v) = table.get(id).filter(|v| v.as_i64().unwrap_or(0) > 0) {
                ranks.insert(id.clone(), v.clone());
            }
        }
        return (Outcome::Answer(json!({"v": 1, "space": space, "ranks": ranks})), now_ms());
    }
    // a refused session (401) is renewed for the NEXT read, which the cache schedules through the 2 s gap
    let Some(session) = own_session(server, base, renew).await else { return (Outcome::Failed, now_ms()) };
    let sent = now_ms();
    let (st, body) = get_ranks(base, &session, space, ids).await;
    let o = match st {
        200 => Outcome::Answer(body),
        503 => Outcome::Off,
        401 => Outcome::Refused,
        _ => {
            eprintln!("[prestige] ranks: HTTP {st} ({} ids)", ids.len());
            Outcome::Failed
        }
    };
    (o, sent)
}

async fn run() {
    loop {
        let now = now_ms();
        let (steps, acct, stale) = {
            let mut e = lock();
            current_me(&mut e);
            // contexts a label shows whose space is unknown or older than 5 min (a failed read included): again
            let stale: Vec<String> = e
                .interest
                .keys()
                .filter(|c| !e.resolving.contains_key(*c))
                .filter(|c| e.ctx.get(*c).is_none_or(|(_, at)| now.saturating_sub(*at) >= SPACE_TTL_MS))
                .cloned()
                .collect();
            (e.cache.due(now), e.acct_gen, stale)
        };
        for c in stale {
            resolve_soon(&c);
        }
        for step in steps {
            match step {
                Step::Base { owner } => spawn(async move {
                    let b = fetch_base(&owner).await;
                    {
                        let mut e = lock();
                        if e.acct_gen != acct {
                            return; // an older account's request
                        }
                        e.cache.finish_base(&owner, b, now_ms());
                    }
                    publish();
                    wake().notify_one();
                }),
                Step::Ranks { scope, base, ids, sent_at: _, renew } => spawn(async move {
                    let (o, sent_at) = read_ranks(&scope, &base, &ids, renew).await;
                    {
                        let mut e = lock();
                        if e.acct_gen != acct {
                            return;
                        }
                        e.cache.finish_ranks(&scope, &ids, sent_at, o, now_ms());
                    }
                    publish();
                    wake().notify_one();
                }),
            }
        }
        publish();
        let next = lock().cache.next_at(now_ms()).unwrap_or(u64::MAX);
        let wait = next.saturating_sub(now_ms()).clamp(10, TICK_MS);
        let _ = tokio::time::timeout(Duration::from_millis(wait), wake().notified()).await;
    }
}

// ───────────────────────────────────── the QML side ─────────────────────────────────────

impl qobject::PrestigeController {
    pub fn start(self: Pin<&mut Self>) {
        if std::env::var_os("TINHAT_VOD_SERVICE").is_some() {
            return; // the recorder draws no names
        }
        if let Ok(mut s) = sink().lock() {
            *s = Some(self.qt_thread());
        }
        let first = {
            let mut e = lock();
            let f = !e.started;
            e.started = true;
            f
        };
        if first {
            eprintln!("[prestige] engine started{}", if fixture().is_some() { " (lab fixture)" } else { "" });
            spawn(run());
        }
        publish();
    }

    pub fn want(self: Pin<&mut Self>, mxid: &QString, context_id: &QString) {
        want_inner(&mxid.to_string(), &context_id.to_string());
    }

    pub fn forget(self: Pin<&mut Self>, mxid: &QString, context_id: &QString) {
        forget_inner(&mxid.to_string(), &context_id.to_string());
    }

    pub fn push(self: Pin<&mut Self>, server: &QString, frame_json: &QString) -> bool {
        match serde_json::from_str::<Value>(&frame_json.to_string()) {
            Ok(v) => apply_push(&server.to_string(), &v),
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyphs_for_every_tier() {
        let want = [
            (0, None),
            (1, Some("prestige-symbolic")),
            (2, Some("prestige-filled-symbolic")),
            (4, Some("prestige-filled-symbolic")),
            (5, Some("prestige-ring-symbolic")),
            (8, Some("prestige-star-symbolic")),
            (13, Some("prestige-star-symbolic")),
        ];
        for (rank, g) in want {
            let look = prestige_tier(rank);
            assert_eq!((look.tier > 0).then(|| glyph_for(&look)), g, "rank {rank}");
        }
    }

    #[test]
    fn play_base_needs_play_exactly_true() {
        let ok = json!({"dev.tinhat.server": {"api": "https://s.test/api", "features": {"play": true}}});
        assert_eq!(play_base_from_well_known(&ok).as_deref(), Some("https://s.test/api/play/v1"));
        let slash = json!({"dev.tinhat.server": {"api": "https://s.test/api/", "features": {"play": true}}});
        assert_eq!(play_base_from_well_known(&slash).as_deref(), Some("https://s.test/api/play/v1"));
        for bad in [
            json!({"dev.tinhat.server": {"api": "https://s.test/api", "features": {"play": 1}}}),
            json!({"dev.tinhat.server": {"api": "https://s.test/api", "features": {"play": "true"}}}),
            json!({"dev.tinhat.server": {"api": "https://s.test/api", "features": {}}}),
            json!({"dev.tinhat.server": {"api": "https://s.test/api"}}),
            json!({"dev.tinhat.server": {"api": "ftp://s.test/api", "features": {"play": true}}}),
            json!({"dev.tinhat.server": {"features": {"play": true}}}),
            json!({"m.homeserver": {"base_url": "https://s.test"}}),
        ] {
            assert_eq!(play_base_from_well_known(&bad), None, "{bad}");
        }
    }

    /// Prestige.qml's generated glyph block = the pack's 24 px files, byte for byte (the generator wrote both; this
    /// catches a hand edit of either side), and the other sizes are the same drawing.
    #[test]
    fn qml_glyphs_are_the_packs() {
        let qml = include_str!("../qml/Prestige.qml");
        let pack: [(&str, &str); 4] = [
            ("prestige-symbolic", include_str!("../assets/iconpack/theme/tinhat/24/actions/prestige-symbolic.svg")),
            ("prestige-filled-symbolic", include_str!("../assets/iconpack/theme/tinhat/24/actions/prestige-filled-symbolic.svg")),
            ("prestige-ring-symbolic", include_str!("../assets/iconpack/theme/tinhat/24/actions/prestige-ring-symbolic.svg")),
            ("prestige-star-symbolic", include_str!("../assets/iconpack/theme/tinhat/24/actions/prestige-star-symbolic.svg")),
        ];
        for (name, file) in pack {
            let key = format!("        \"{name}\": ");
            let line = qml.lines().find(|l| l.starts_with(&key)).unwrap_or_else(|| panic!("{name} not in Prestige.qml"));
            let lit = line[key.len()..].trim_end_matches(',');
            let in_qml: String = serde_json::from_str(lit).unwrap();
            assert_eq!(in_qml, file, "{name}");
            assert!(file.contains("fill:currentColor") && file.contains("#eff0f1"), "{name}: the pack tint convention");
        }
        let small = include_str!("../assets/iconpack/theme/tinhat/16/actions/prestige-star-symbolic.svg");
        assert_eq!(small.replace("width=\"16\" height=\"16\"", "width=\"24\" height=\"24\""), pack[3].1);
    }

    #[test]
    fn ranks_query_is_encoded() {
        assert_eq!(urlencode("@a:example.org:8448"), "%40a%3Aexample.org%3A8448");
    }
}
