//! layout_store.rs — your layouts across your devices (11.0 edit mode; EDITMODE-UI.md §13, §16, LAYOUT.md §9): the
//! desktop twin of the web's `features/editmode/layoutDoc.ts` + `layoutSync.ts`. The rules are the engine crate's
//! device protocol (`tinhat_layout::Device`: counter, chains, known, own, lost, the newer-format latch — the port of
//! the web's reference, 1,135 shared vectors); this file is the plumbing around it: the per-account device state on
//! disk (written BEFORE any PUT that carries a save), the account-data read at the first sync and the events of every
//! later sync (bridge.rs → `on_sync`), the PUTs in the background with retries and re-asserts, and the view the
//! LayoutController hands QML (`view(active)`: which layout renders, its forms, the named layouts, previous, conflict).
//!
//! What renders (§16): `project(merge(raw, known), layout-active)` → its desktop / phone forms (repaired); no document
//! entry yet → QML's LayoutStore falls back to the device prefs `layout-desktop` / `layout-phone` (the buttons step),
//! which the first save of `main` copies verbatim; a device-local reset override (§9, pref `layout-override`) is
//! applied by LayoutStore.qml on top.
//!
//! Gate mode (`TINHAT_GUI_LAYOUTSYNC=0`, and — unless `TINHAT_GUI_LAYOUTSYNC=1` — whenever `TINHAT_GUI_THEMESYNC=0`: the gates
//! run on a test account, which must never get synced data): the account is neither read nor written; saves live in this
//! run's memory only.
//!
//! Threading: the engine keeps the device state as canonical JSON TEXT (the engine's `J` values are single-threaded
//! `Rc`s), so the one mutex here holds only `Send` data; every protocol step parses, runs, and writes the text back.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use matrix_sdk::ruma::events::{AnyGlobalAccountDataEvent, GlobalAccountDataEventType};
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::Client;
use tinhat_layout::json::{mk, read_json, J};
use tinhat_layout::{merge, project, Device};

pub const LAYOUTS_TYPE: &str = "dev.tinhat.layouts";
/// Every account-data round trip is bounded.
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// Sync on (the default) or gate mode: `TINHAT_GUI_LAYOUTSYNC=0`; unset, the themes' gate switch `TINHAT_GUI_THEMESYNC=0`
/// turns it off too (every existing gate runs with it); `TINHAT_GUI_LAYOUTSYNC=1` forces it on (the sync check on test2).
pub fn sync_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| match std::env::var("TINHAT_GUI_LAYOUTSYNC").map(|v| v.trim().to_string()) {
        Ok(v) if v == "0" => false,
        Ok(v) if v == "1" => true,
        _ => std::env::var("TINHAT_GUI_THEMESYNC").map(|v| v.trim() != "0").unwrap_or(true),
    })
}

fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

struct Engine {
    /// session generation this state belongs to (crate::session::generation)
    gen: u64,
    user: String,
    /// the device protocol's state (`Device::to_store`, canonical text); empty = none (signed out)
    device: String,
    /// the first sync of this session read the document (nothing goes out before: the read decides first)
    synced: bool,
    /// the next PUT is a re-assert: it waits 0–2 s by the device name
    reassert: bool,
    attempt: i64,
    worker: bool,
    /// layout ids whose save of this device was replaced (the conflict notice), for QML to show once
    lost: Vec<String>,
    /// edit sessions begun on the sync device (LAYOUT §9.7): layout id → the start entry (canonical text, "" = none).
    /// While one is open, only its Done saves that layout ("busy" for every other save — one owner per layout).
    sessions: Vec<(String, String)>,
    /// bumps on every change QML should re-read
    rev: u64,
}

impl Engine {
    fn fresh(gen: u64) -> Engine {
        Engine { gen, user: String::new(), device: String::new(), synced: false, reassert: false, attempt: 0, worker: false, lost: Vec::new(), sessions: Vec::new(), rev: 0 }
    }
}

fn engine() -> &'static Mutex<Engine> {
    static E: OnceLock<Mutex<Engine>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(Engine::fresh(crate::session::generation())))
}

fn lock() -> std::sync::MutexGuard<'static, Engine> {
    engine().lock().unwrap_or_else(|p| p.into_inner())
}

// ---- the device state (text ↔ Device) ------------------------------------------------------------------------------

fn device_of(e: &Engine) -> Option<Device> {
    if e.device.is_empty() {
        return None;
    }
    let mut d = read_json(&e.device).and_then(|v| Device::from_store(&v))?;
    for (id, start) in &e.sessions {
        d.set_session_of(&Some(id.clone()), read_json(start));
    }
    Some(d)
}

/// Carry the device's begun sessions back into the engine (after a step that may have used or ended one).
fn keep_sessions(e: &mut Engine, d: &Device) {
    let ids: Vec<String> = e.sessions.iter().map(|(id, _)| id.clone()).collect();
    e.sessions = ids
        .into_iter()
        .filter_map(|id| d.session_of(&Some(id.clone())).map(|st| (id, st.map(|j| j.canon()).unwrap_or_default())))
        .collect();
}

/// Store a device back (and on disk when it changed).
fn put_device(e: &mut Engine, d: &Device) {
    let text = d.to_store().canon();
    if text != e.device {
        e.device = text;
        save_state(e);
        e.rev += 1;
    }
}

fn dir() -> PathBuf {
    crate::platform::app_data_dir().join("layoutsync")
}

fn state_file(user: &str) -> PathBuf {
    let safe: String = user.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect();
    dir().join(format!("{safe}.json"))
}

fn save_state(e: &Engine) {
    if !sync_enabled() || e.user.is_empty() || e.device.is_empty() || cfg!(test) {
        return;
    }
    let _ = std::fs::create_dir_all(dir());
    let path = state_file(&e.user);
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, &e.device).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// A new device name: the Matrix device id (no `#`, ≤ 48 code points) + a random suffix (LAYOUT §9.7 durability: a
/// device without saved state never reuses an old name whose counter it has lost) — the web's layoutDoc.ts `load()`.
fn new_device_name(matrix_device: &str) -> String {
    let base: String = matrix_device.replace('#', "").chars().take(48).collect();
    let base = if base.is_empty() { "desktop".to_string() } else { base };
    let mut x = (now_ms() as u64) ^ ((std::process::id() as u64) << 32) ^ 0x9e37_79b9_7f4a_7c15;
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut s = String::new();
    for _ in 0..6 {
        s.push(digits[(x % 36) as usize] as char);
        x /= 36;
    }
    format!("{base}-{s}")
}

/// The signed-in session, attached to the engine (a new account or a new session starts from that account's state on
/// disk). Returns false when nobody is signed in.
fn ensure_session(e: &mut Engine) -> bool {
    let Some(client) = crate::session::client() else { return false };
    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let gen = crate::session::generation();
    if me.is_empty() {
        return false;
    }
    if e.user != me || e.gen != gen {
        *e = Engine::fresh(gen);
        e.user = me;
        let stored = if sync_enabled() && !cfg!(test) { std::fs::read_to_string(state_file(&e.user)).ok() } else { None };
        let dev = stored.and_then(|t| read_json(&t)).and_then(|v| Device::from_store(&v)).unwrap_or_else(|| {
            let md = client.device_id().map(|d| d.to_string()).unwrap_or_default();
            Device::new(&new_device_name(&md))
        });
        e.device = dev.to_store().canon();
        e.rev += 1;
    }
    true
}

// ---- the view QML renders ------------------------------------------------------------------------------------------

fn tuple_of(e: &J) -> J {
    let rev = e.get("rev").and_then(|v| v.int_val()).unwrap_or(0);
    let at = e.get("savedAt").and_then(|v| v.int_val()).unwrap_or(0);
    let by = e.get("by").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    J::arr(vec![J::int(rev), J::int(at), J::s(&by)])
}

fn name_of(doc: Option<&J>, id: &str) -> Option<String> {
    let ls = doc?.get("layouts").filter(J::is_arr)?;
    ls.items().into_iter().find(|e| e.is_obj() && e.is("id", id)).and_then(|e| e.get("name")).and_then(|n| n.as_str().map(str::to_string))
}

/// What this device renders from its synced state for the active layout id (the web's layoutDoc.ts `recompute`, minus
/// the override and the no-document fallback, which LayoutStore.qml applies): `{signedIn, sync, synced, rev, id, name,
/// desktop, phone, source: "doc" | "none", readOnly, layouts: [{id, name}], hasPrevious, previous, conflict, tuple}`.
pub fn view(active: &str) -> String {
    let mut e = lock();
    let signed = ensure_session(&mut e);
    let dev = device_of(&e);
    let doc = dev.as_ref().map(|d| if d.read_only() { d.raw.clone().unwrap_or_default() } else { merge(d.raw.as_ref(), &d.known, None).0 });
    let doc = doc.filter(J::is_obj);
    let act = if active.is_empty() { "main" } else { active };
    let p = project(doc.as_ref(), Some(act));
    let has_entry = !p.layouts.is_empty();
    let layouts = J::arr(
        p.layouts.iter().map(|id| mk(vec![("id", J::s(id)), ("name", J::s(&name_of(doc.as_ref(), id).unwrap_or_else(|| id.clone())))])).collect(),
    );
    let cur = if has_entry { dev.as_ref().and_then(|d| d.best(&Some(p.active.clone()))) } else { None };
    let previous = cur.as_ref().and_then(|c| c.get("previous")).filter(|p| p.is_obj() && p.get("desktop").is_some_and(|d| d.is_obj()) && p.get("phone").is_some_and(|d| d.is_obj()));
    let conflict = !p.conflicts.is_empty() || dev.as_ref().is_some_and(|d| d.lost.contains_key(&p.active));
    let out = mk(vec![
        ("signedIn", J::Bool(signed)),
        ("sync", J::Bool(sync_enabled())),
        ("synced", J::Bool(e.synced)),
        ("rev", J::int(e.rev as i64)),
        ("id", J::s(&p.active)),
        ("name", J::s(&p.name)),
        ("desktop", p.desktop.clone()),
        ("phone", p.phone.clone()),
        ("source", J::s(if has_entry { "doc" } else { "none" })),
        ("readOnly", J::Bool(p.read == "newer" || dev.as_ref().is_some_and(Device::read_only))),
        ("layouts", layouts),
        ("hasPrevious", J::Bool(previous.is_some())),
        ("previous", previous.map(|p| p.deep()).unwrap_or(J::Null)),
        ("conflict", J::Bool(conflict)),
        ("tuple", cur.as_ref().map(tuple_of).unwrap_or_else(|| J::arr(vec![J::int(0), J::int(0), J::s("")]))),
        ("lost", J::arr(e.lost.iter().map(|s| J::s(s)).collect())),
    ]);
    out.canon()
}

/// One save (Keep for all devices, Restore previous, New, Rename, Delete, a row edit — EDITMODE-UI §16): `saved`,
/// `noop`, `read-only`, `busy` (an edit session owns that layout), `signed-out` or the engine's error. A saved edit is
/// written to disk now and PUT in the background. `from_session` = the Done of the session that owns the layout.
pub fn save(id: &str, edit_json: &str, from_session: bool) -> String {
    let mut e = lock();
    if !ensure_session(&mut e) {
        return "signed-out".into();
    }
    if !from_session && e.sessions.iter().any(|(x, _)| x == id) {
        return "busy".into();
    }
    let Some(mut d) = device_of(&e) else { return "signed-out".into() };
    let Some(edit) = read_json(edit_json).filter(J::is_obj) else { return "bad-op: bad request".into() };
    let r = d.save(&Some(id.to_string()), Some(edit), Some(J::Num(now_ms())));
    keep_sessions(&mut e, &d);
    if r == "saved" {
        put_device(&mut e, &d);
        e.rev += 1;
        drop(e);
        kick_put();
        publish();
    }
    r
}

/// An edit session opens on layout `id` (LAYOUT §9.7): the sync device remembers the entry it starts from, so a save
/// another device makes meanwhile is kept as `previous` at Done, never claimed as included.
pub fn begin(id: &str) -> String {
    let mut e = lock();
    if !ensure_session(&mut e) {
        return "signed-out".into();
    }
    let Some(mut d) = device_of(&e) else { return "signed-out".into() };
    let r = d.begin(&Some(id.to_string()));
    let st = d.session_of(&Some(id.to_string())).flatten().map(|j| j.canon()).unwrap_or_default();
    e.sessions.retain(|(x, _)| x != id);
    e.sessions.push((id.to_string(), st));
    r
}

/// The session on `id` ended without a save (Cancel, a Done that changed nothing): its begun start is forgotten.
pub fn end(id: &str) {
    let mut e = lock();
    e.sessions.retain(|(x, _)| x != id);
}

/// Restore mine (`take`) / Dismiss of the active layout's first conflict (§13). A lost hint without a conflict in the
/// document is cleared.
pub fn resolve(id: &str, take: bool) -> String {
    let mut e = lock();
    if !ensure_session(&mut e) {
        return "signed-out".into();
    }
    let Some(mut d) = device_of(&e) else { return "signed-out".into() };
    if e.sessions.iter().any(|(x, _)| x == id) {
        return "busy".into();
    }
    let r = d.resolve(&Some(id.to_string()), Some(J::Num(now_ms())), take, 0.0);
    if r == "saved" {
        put_device(&mut e, &d);
        drop(e);
        kick_put();
        publish();
    } else if r == "none" && d.lost.contains_key(id) {
        d.lost.remove(id);
        put_device(&mut e, &d);
        drop(e);
        publish();
    }
    r
}

/// The conflict notices QML has not shown yet (layout ids), drained.
pub fn take_lost() -> Vec<String> {
    let mut e = lock();
    std::mem::take(&mut e.lost)
}

/// The device protocol's state (the web's `__twEdit.device()`; gates and the sync check read it).
pub fn debug_state() -> String {
    let e = lock();
    device_of(&e).map(|d| d.state_json().canon()).unwrap_or_else(|| "null".into())
}

// ---- the syncs -----------------------------------------------------------------------------------------------------

async fn read_account_data(client: &Client, ty: &str) -> Option<J> {
    let raw = client.account().account_data_raw(GlobalAccountDataEventType::from(ty)).await.ok().flatten()?;
    read_json(raw.json().get())
}

async fn write_account_data(client: &Client, ty: &str, text: String) -> Result<(), String> {
    let raw_value = serde_json::value::RawValue::from_string(text).map_err(|e| e.to_string())?;
    let raw: Raw<matrix_sdk::ruma::events::AnyGlobalAccountDataEventContent> = Raw::from_json(raw_value);
    client.account().set_account_data_raw(GlobalAccountDataEventType::from(ty), raw).await.map(|_| ()).map_err(|e| e.to_string())
}

fn session_is(user: &str, gen: u64) -> bool {
    crate::session::generation() == gen && crate::session::client().and_then(|c| c.user_id().map(|u| u.to_string())).as_deref() == Some(user)
}

/// A document arrived (the first sync's read, or an event — the echo of this device's own PUT included).
fn receive(e: &mut Engine, content: Option<&J>) {
    let Some(mut d) = device_of(e) else { return };
    let r = d.receive(content);
    eprintln!("[layoutsync] receive: {r}");
    for part in r.split(',') {
        if let Some(id) = part.trim().strip_prefix("lost ") {
            if !e.lost.iter().any(|x| x == id) {
                e.lost.push(id.to_string());
            }
        }
    }
    if d.want_put && !d.read_only() {
        e.reassert = true;
    }
    put_device(e, &d);
    e.rev += 1;
}

/// bridge.rs's sync loop, after every successful pass (like theme_store.rs `on_sync`).
pub async fn on_sync(client: &Client, account_data: &[Raw<AnyGlobalAccountDataEvent>]) {
    if !sync_enabled() {
        return;
    }
    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let gen = crate::session::generation();
    let first = {
        let mut e = lock();
        if !ensure_session(&mut e) || e.user != me {
            return;
        }
        !e.synced
    };
    if first {
        let doc = read_account_data(client, LAYOUTS_TYPE).await;
        let mut e = lock();
        if !session_is(&me, gen) || e.user != me || e.gen != gen || e.synced {
            return;
        }
        if doc.is_some() {
            receive(&mut e, doc.as_ref());
        }
        e.synced = true;
        e.rev += 1;
    } else {
        let mut e = lock();
        if e.user != me || e.gen != gen {
            return;
        }
        let mut any = false;
        for raw in account_data {
            let Ok(Some(ty)) = raw.get_field::<String>("type") else { continue };
            if ty != LAYOUTS_TYPE {
                continue;
            }
            let content = read_json(raw.json().get()).and_then(|v| v.get("content"));
            receive(&mut e, content.as_ref());
            any = true;
        }
        if !any {
            return;
        }
    }
    kick_put();
    publish();
}

/// Sign-out: this account's in-memory state is forgotten (the state file stays with the account on this computer, like
/// the web's localStorage copy — its counter keeps later saves ordered).
pub fn on_sign_out() {
    let mut e = lock();
    *e = Engine::fresh(crate::session::generation());
    e.rev += 1;
    drop(e);
    publish();
}

// ---- PUTs ----------------------------------------------------------------------------------------------------------

/// Start the PUT worker when the device wants a PUT (at most one; retries 1, 2, 4 … s up to 5 min).
fn kick_put() {
    if !sync_enabled() {
        return;
    }
    let go = {
        let mut e = lock();
        let wants = device_of(&e).is_some_and(|d| d.want_put && !d.read_only());
        let go = e.synced && !e.user.is_empty() && wants && !e.worker;
        e.worker |= go;
        go
    };
    if go {
        crate::bridge::runtime().spawn(put_worker());
    }
}

async fn put_worker() {
    loop {
        let delay = {
            let mut e = lock();
            let d = if e.reassert { device_of(&e).map(|d| tinhat_theme::sync::reassert_delay_ms(&d.name)) } else { None };
            e.reassert = false;
            d
        };
        if let Some(ms) = delay.filter(|ms| *ms > 0) {
            tokio::time::sleep(Duration::from_millis(ms as u64)).await;
        }
        // the document to PUT, from a copy: the stored state keeps wantPut until the server accepted it (a crash or a
        // sign-out during the PUT leaves the save queued — EDITMODE-UI §16 durability)
        let (client, text, user, gen) = {
            let mut e = lock();
            let Some(d) = device_of(&e) else {
                e.worker = false;
                return;
            };
            let mut copy = d.clone();
            let mut server: Option<J> = None;
            if copy.put(&mut server) != "put" || server.is_none() {
                e.worker = false;
                return;
            }
            (crate::session::client(), server.expect("a document").canon(), e.user.clone(), e.gen)
        };
        let ok = match client {
            Some(c) if session_is(&user, gen) => match tokio::time::timeout(HTTP_TIMEOUT, write_account_data(&c, LAYOUTS_TYPE, text.clone())).await {
                Ok(Ok(())) => {
                    eprintln!("[layoutsync] PUT {LAYOUTS_TYPE}: ok");
                    true
                }
                Ok(Err(why)) => {
                    eprintln!("[layoutsync] PUT {LAYOUTS_TYPE} failed: {why}");
                    false
                }
                Err(_) => {
                    eprintln!("[layoutsync] PUT {LAYOUTS_TYPE} timed out");
                    false
                }
            },
            _ => false,
        };
        let wait = {
            let mut e = lock();
            if e.user != user || e.gen != gen {
                return; // signed out meanwhile: this session's state is gone
            }
            if ok {
                e.attempt = 0;
                // accepted: the device no longer wants THIS document — unless a save came in meanwhile (its pending
                // document differs), which the next turn puts
                if let Some(mut d) = device_of(&e) {
                    if d.pending().canon() == text {
                        let mut sink: Option<J> = None;
                        d.put(&mut sink);
                        put_device(&mut e, &d);
                    }
                }
                None
            } else {
                let wait = tinhat_theme::sync::retry_delay_ms(e.attempt);
                e.attempt += 1;
                Some(wait)
            }
        };
        if let Some(ms) = wait {
            publish();
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
    }
}

// ---- QML plumbing --------------------------------------------------------------------------------------------------

type Sink = cxx_qt::CxxQtThread<crate::layout_ctl::qobject::LayoutController>;

fn sink() -> &'static Mutex<Option<Sink>> {
    static S: OnceLock<Mutex<Option<Sink>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

/// LayoutController.startSync(): QML hears about every later change through `publish` (its `storeRev` bumps).
pub fn attach_sink(s: Sink) {
    *sink().lock().unwrap_or_else(|p| p.into_inner()) = Some(s);
}

/// Tell QML to re-read the view (queued onto the GUI thread).
pub fn publish() {
    let Some(qt) = sink().lock().ok().and_then(|s| s.clone()) else { return };
    let _ = qt.queue(|o| crate::layout_ctl::bump(o));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_names_are_fresh_and_valid() {
        let a = new_device_name("ABCDEF#12");
        assert!(a.starts_with("ABCDEF12-") && a.chars().count() == 9 + 6, "{a}");
        assert!(!a.contains('#'));
        let long = new_device_name(&"x".repeat(80));
        assert!(long.chars().count() <= 64, "{long}");
        assert!(new_device_name("").starts_with("desktop-"));
    }

    #[test]
    fn device_state_round_trips_through_text() {
        let mut d = Device::new("dev-a");
        let classic = tinhat_layout::classic_desktop();
        let phone = tinhat_layout::classic_phone();
        let edit = mk(vec![("name", J::s("Main")), ("desktop", classic), ("phone", phone)]);
        assert_eq!(d.save(&Some("main".into()), Some(edit), Some(J::int(1000))), "saved");
        let text = d.to_store().canon();
        let back = Device::from_store(&read_json(&text).unwrap()).unwrap();
        assert_eq!(back.to_store().canon(), text);
        assert_eq!(back.state_json().canon(), d.state_json().canon());
        // a second save on the restored device continues the same chain (counter 2)
        let mut b = back;
        let edit2 = mk(vec![("name", J::s("Renamed")), ("desktop", tinhat_layout::classic_desktop()), ("phone", tinhat_layout::classic_phone())]);
        assert_eq!(b.save(&Some("main".into()), Some(edit2), Some(J::int(2000))), "saved");
        assert_eq!(b.counter, 2);
    }
}
