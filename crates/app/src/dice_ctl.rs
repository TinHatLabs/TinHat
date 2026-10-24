//! DiceController — widgets W3 (2026-09-28): the Dice widget's device side (contract WIDGETS.md §16; the web's
//! `features/widgets/dice/store.ts`).
//!
//! * **Rolling:** the shared parser (tinhat_widgets::dice), every die from the OS CSPRNG (`rand::rngs::OsRng`, getrandom)
//!   through the shared rejection sampling (`die_from_u32` — every face exactly as likely). Rolls are made on the roller's
//!   device: an honour system, like dice on a real table (receivers re-add the numbers, they can't re-roll them).
//! * **Room rolls:** `rollForRoom(expr)` hands QML the shared message content (m.notice + HTML + `dev.tinhat.dice`), which
//!   the open room's RoomController sends through the local-echo path (`sendDice`: encrypted in an encrypted room,
//!   pending / failed rows with Retry like any message).
//! * **Private rolls:** this device only, never sent — `<data>/dice.json` `{me, list}` (the shared normalize / add; another
//!   account's file is ignored and replaced on the next write). "Clear private rolls" empties it.
//! * **The history:** `history(roomRowsJson, limit)` = the shared `dice_history` over the open room's dice rows
//!   (RoomController.diceJson — the loaded timeline's dice messages, decrypted) and the private list.
//! * **The landing:** every roll made here bumps `justKey` / `justJson` {expr, rolls, at}; the card that shows that roll
//!   lands once (DiceCard.qml).

use core::pin::Pin;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use rand::RngCore;
use serde_json::{json, Value};
use tinhat_widgets::dice::{
    add_private, dice_content, dice_history, dice_summary, faces_of, modifier_of, normalize_private, parse_dice, private_json, read_dice,
    roll_with, with_advantage, DiceEntry, DiceResult, PrivateRoll, DICE_FIELD,
};

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
        #[qproperty(QString, private_json)] // QML: privateJson — this device's private rolls [{id, ts, expr, rolls, total}], newest first
        #[qproperty(i32, just_key)] //         QML: justKey — bumps with every roll made here (the card's one landing)
        #[qproperty(QString, just_json)] //    QML: justJson — that roll {expr, rolls, at}
        type DiceController = super::DiceControllerRust;

        /// QML: start() — load this device's private list and follow it (idempotent).
        #[qinvokable]
        fn start(self: Pin<&mut DiceController>);
        /// QML: stop() — the widget went away.
        #[qinvokable]
        fn stop(self: Pin<&mut DiceController>);
        /// QML: rollForRoom(expr) — roll; → {ok: true, content} (the message RoomController.sendDice sends) | {ok: false, message}.
        #[qinvokable]
        fn roll_for_room(self: Pin<&mut DiceController>, expr: &QString) -> QString;
        /// QML: rollPrivate(expr) — roll into the private list; → "" or the expression's friendly error.
        #[qinvokable]
        fn roll_private(self: Pin<&mut DiceController>, expr: &QString) -> QString;
        /// QML: clearPrivate() — "Clear private rolls".
        #[qinvokable]
        fn clear_private(self: Pin<&mut DiceController>);
        /// QML: history(roomRowsJson, limit) — the widget's list: [{id, ts, sender, private, result, faces, modifier}].
        #[qinvokable]
        fn history(self: &DiceController, room_rows_json: &QString, limit: i32) -> QString;
        /// QML: summary(entryJson, who) — the Small line (dice_summary; entry "" = none).
        #[qinvokable]
        fn summary(self: &DiceController, entry_json: &QString, who: &QString) -> QString;
        /// QML: withAdvantage(expr, adv) — the chips' expression.
        #[qinvokable]
        fn with_advantage(self: &DiceController, expr: &QString, adv: bool) -> QString;
        /// QML: canonical(expr) — the expression as the roll writes it ("" when it doesn't parse).
        #[qinvokable]
        fn canonical(self: &DiceController, expr: &QString) -> QString;
        /// QML: sampleEntry(id, sender, ts, expr, rollsJson, private) — a sample history entry (the gallery / gate states).
        #[qinvokable]
        fn sample_entry(self: &DiceController, id: &QString, sender: &QString, ts: f64, expr: &QString, rolls_json: &QString, is_private: bool) -> QString;
    }

    impl cxx_qt::Threading for DiceController {}
}

type QtThread = cxx_qt::CxxQtThread<qobject::DiceController>;

pub struct DiceControllerRust {
    private_json: QString,
    just_key: i32,
    just_json: QString,
    id: u64,
}

impl Default for DiceControllerRust {
    fn default() -> Self {
        Self { private_json: QString::from("[]"), just_key: 0, just_json: QString::default(), id: 0 }
    }
}

// ── the store (process-wide: the stack's widget and the gallery's preview share it) ─────────────────────────────────────

#[derive(Default)]
struct Store {
    loaded_for: String,
    list: Vec<PrivateRoll>,
    just_key: i32,
    just: String,
    views: HashMap<u64, QtThread>,
}

fn store() -> &'static Mutex<Store> {
    static S: OnceLock<Mutex<Store>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Store::default()))
}

pub(crate) fn file() -> std::path::PathBuf {
    crate::bridge::data_dir().join("dice.json")
}

/// Sign-out (signout.rs, which removes the file): this account's private rolls go; the views stay registered.
pub(crate) fn forget_session() {
    let mut s = store().lock().unwrap_or_else(|p| p.into_inner());
    let views = std::mem::take(&mut s.views);
    *s = Store { views, ..Store::default() };
}

fn me() -> String {
    crate::session::client().and_then(|c| c.user_id().map(|u| u.to_string())).unwrap_or_default()
}

fn ensure_loaded(st: &mut Store) {
    let me = me();
    if me.is_empty() || st.loaded_for == me {
        return;
    }
    st.loaded_for = me.clone();
    let saved: Option<Value> = std::fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(&t).ok());
    st.list = match saved {
        Some(v) if v.get("me").and_then(Value::as_str) == Some(me.as_str()) => normalize_private(v.get("list").unwrap_or(&Value::Null)),
        _ => Vec::new(),
    };
}

fn persist(st: &Store) {
    if st.loaded_for.is_empty() {
        return;
    }
    let v = json!({ "me": st.loaded_for, "list": private_json(&st.list) });
    let p = file();
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, v.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

fn push(st: &Store) {
    let list = private_json(&st.list).to_string();
    for qt in st.views.values() {
        let (list, key, just) = (list.clone(), st.just_key, st.just.clone());
        let _ = qt.queue(move |mut o| {
            o.as_mut().set_private_json(QString::from(&list));
            o.as_mut().set_just_json(QString::from(&just));
            o.as_mut().set_just_key(key);
        });
    }
}

fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

/// Roll `expr` with the OS CSPRNG (every die by the shared rejection sampling).
fn roll_os(expr: &str) -> Result<DiceResult, String> {
    let p = parse_dice(expr).map_err(|e| e.message)?;
    let mut rng = rand::rngs::OsRng;
    Ok(roll_with(&p, || rng.next_u32()))
}

/// A card's JSON for QML: the result + its faces + the modifier text.
pub fn card_json(r: &DiceResult) -> Value {
    let mut o = r.to_json();
    o["faces"] = faces_of(r);
    o["modifier"] = Value::String(modifier_of(r));
    o
}

fn entry_json(e: &DiceEntry) -> Value {
    let mut o = e.to_json();
    o["result"] = card_json(&e.result);
    o
}

fn note_just(st: &mut Store, r: &DiceResult) {
    st.just_key += 1;
    st.just = json!({ "expr": r.expr, "rolls": r.rolls, "at": now_ms() }).to_string();
}

impl qobject::DiceController {
    fn view_id(mut self: Pin<&mut Self>) -> u64 {
        if self.rust().id == 0 {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            self.as_mut().rust_mut().id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        self.rust().id
    }

    pub fn start(mut self: Pin<&mut Self>) {
        let id = self.as_mut().view_id();
        let qt = self.qt_thread();
        if let Ok(mut st) = store().lock() {
            ensure_loaded(&mut st);
            st.views.insert(id, qt);
            push(&st);
        }
    }

    pub fn stop(mut self: Pin<&mut Self>) {
        let id = self.as_mut().view_id();
        if let Ok(mut st) = store().lock() {
            st.views.remove(&id);
        }
    }

    pub fn roll_for_room(self: Pin<&mut Self>, expr: &QString) -> QString {
        let out = match roll_os(&expr.to_string()) {
            Ok(r) => {
                if let Ok(mut st) = store().lock() {
                    note_just(&mut st, &r);
                    push(&st);
                }
                json!({ "ok": true, "content": dice_content(&r) })
            }
            Err(message) => json!({ "ok": false, "message": message }),
        };
        QString::from(&out.to_string())
    }

    pub fn roll_private(self: Pin<&mut Self>, expr: &QString) -> QString {
        match roll_os(&expr.to_string()) {
            Ok(r) => {
                if let Ok(mut st) = store().lock() {
                    ensure_loaded(&mut st);
                    if st.loaded_for.is_empty() {
                        return QString::from("Not signed in");
                    }
                    let id = format!("p-{}-{:x}", now_ms() as u64, rand::rngs::OsRng.next_u32());
                    let roll = PrivateRoll { id, ts: now_ms(), expr: r.expr.clone(), rolls: r.rolls.clone(), total: r.total };
                    st.list = add_private(&st.list, &roll);
                    note_just(&mut st, &r);
                    persist(&st);
                    push(&st);
                }
                QString::default()
            }
            Err(message) => QString::from(&message),
        }
    }

    pub fn clear_private(self: Pin<&mut Self>) {
        if let Ok(mut st) = store().lock() {
            ensure_loaded(&mut st);
            if st.list.is_empty() {
                return;
            }
            st.list.clear();
            persist(&st);
            push(&st);
        }
    }

    pub fn history(&self, room_rows_json: &QString, limit: i32) -> QString {
        let rows: Vec<Value> = serde_json::from_str::<Value>(&room_rows_json.to_string()).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
        // the room rows carry the verified field; the shared history re-reads it like any message content
        let events: Vec<Value> = rows
            .iter()
            .map(|r| {
                json!({ "id": r.get("id").cloned().unwrap_or_default(), "sender": r.get("sender").cloned().unwrap_or_default(),
                        "ts": r.get("ts").cloned().unwrap_or_default(), "content": { DICE_FIELD: r.get("field").cloned().unwrap_or_default() } })
            })
            .collect();
        let private = store().lock().map(|st| st.list.clone()).unwrap_or_default();
        let list = dice_history(&events, &private, limit as f64);
        if std::env::var("TINHAT_DICE_DEBUG").is_ok() {
            eprintln!("[dice] history: {} room rows, {} private → {} entries", rows.len(), private.len(), list.len());
        }
        QString::from(&Value::Array(list.iter().map(entry_json).collect()).to_string())
    }

    pub fn summary(&self, entry_json: &QString, who: &QString) -> QString {
        let v: Value = serde_json::from_str(&entry_json.to_string()).unwrap_or(Value::Null);
        let entry = if v.is_object() {
            let r = &v["result"];
            read_dice(&json!({ DICE_FIELD: { "v": 1, "expr": r["expr"], "rolls": r["rolls"], "total": r["total"] } })).map(|result| DiceEntry {
                id: v["id"].as_str().unwrap_or("").into(),
                ts: v["ts"].as_f64().unwrap_or(0.0),
                sender: v["sender"].as_str().unwrap_or("").into(),
                private: v["private"] == Value::Bool(true),
                result,
            })
        } else {
            None
        };
        QString::from(&dice_summary(entry.as_ref(), &who.to_string()))
    }

    pub fn with_advantage(&self, expr: &QString, adv: bool) -> QString {
        QString::from(&with_advantage(&expr.to_string(), adv))
    }

    pub fn canonical(&self, expr: &QString) -> QString {
        QString::from(&parse_dice(&expr.to_string()).map(|p| p.expr).unwrap_or_default())
    }

    pub fn sample_entry(&self, id: &QString, sender: &QString, ts: f64, expr: &QString, rolls_json: &QString, is_private: bool) -> QString {
        let rolls: Value = serde_json::from_str(&rolls_json.to_string()).unwrap_or(Value::Null);
        let Some(result) = parse_dice(&expr.to_string()).ok().and_then(|p| rolls.as_array().and_then(|a| tinhat_widgets::dice::evaluate_dice(&p, a))) else {
            return QString::default();
        };
        let e = DiceEntry { id: id.to_string(), ts, sender: if is_private { String::new() } else { sender.to_string() }, private: is_private, result };
        QString::from(&entry_json(&e).to_string())
    }
}
