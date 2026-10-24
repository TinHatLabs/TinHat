//! NoteController — widgets W1 (2026-09-28): the Sticky note widget's data (contract WIDGETS.md §14).
//!
//! One plain-text note per space: the space room's `dev.tinhat.note` state (key ""), `{text, v: 1}` (≤ 500 code points;
//! `{}` = no note). The author and the time are the state event's own sender and origin_server_ts. Who may edit:
//! `edit_space_note` (docs/RBAC.md §2) — the space's roles when active, else its power levels (roles::effective_perms,
//! the rule every permission-gated UI uses; Matrix enforces the derived level). The shared rules (reading, saving,
//! the counter, the link segments, the Small line) are tinhat_widgets::note, the web's note.ts line for line.
//! The note follows the sync: a change of the note, the power levels or the roles in a shown space re-reads it
//! ([`on_sync`], one line in bridge.rs's sync loop). Nothing is kept on disk.

use core::pin::Pin;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use serde_json::{json, Value};
use tinhat_widgets::note::{note_counter_json, note_first_line, note_of, note_segments_json, note_to_save, NOTE_TYPE};

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
        #[qproperty(QString, space_id)] //   QML: spaceId — the space whose note shows ("" = none)
        #[qproperty(QString, note_json)] //  QML: noteJson — {space, spaceName, text (null = none), sender, name, avatar, ts, canEdit} ("" = not read yet)
        #[qproperty(bool, saving)] //        QML: saving — a Save in flight
        #[qproperty(QString, error)] //      QML: error — the last Save's failure ("" = none)
        type NoteController = super::NoteControllerRust;

        /// QML: show(spaceId) — follow this space's note ("" = none).
        #[qinvokable]
        fn show(self: Pin<&mut NoteController>, space_id: &QString);
        /// QML: save(text) — write the note (note::note_to_save: "" clears it; over 500 code points is refused).
        #[qinvokable]
        fn save(self: Pin<&mut NoteController>, text: &QString);
        /// QML: counter(text) — {count, max, near, over} (note::note_counter).
        #[qinvokable]
        fn counter(self: &NoteController, text: &QString) -> QString;
        /// QML: segments(text) — [{text, url?}] (note::note_segments).
        #[qinvokable]
        fn segments(self: &NoteController, text: &QString) -> QString;
        /// QML: firstLine(text) — the Small line (note::note_first_line).
        #[qinvokable]
        fn first_line(self: &NoteController, text: &QString) -> QString;
        /// QML: close() — the widget went away (stop following).
        #[qinvokable]
        fn close(self: Pin<&mut NoteController>);
    }

    impl cxx_qt::Threading for NoteController {}
}

type QtThread = cxx_qt::CxxQtThread<qobject::NoteController>;

#[derive(Default)]
pub struct NoteControllerRust {
    space_id: QString,
    note_json: QString,
    saving: bool,
    error: QString,
    id: u64,
}

/// view id → (its thread, the space it shows)
fn views() -> &'static Mutex<HashMap<u64, (QtThread, String)>> {
    static V: OnceLock<Mutex<HashMap<u64, (QtThread, String)>>> = OnceLock::new();
    V.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The types whose change re-reads a shown space's note (the note itself, who may edit it).
const WATCH: &[&str] = &[NOTE_TYPE, "m.room.power_levels", "dev.tinhat.role", "dev.tinhat.member_roles", "m.room.name"];

/// From the sync loop: re-read the note of every shown space that saw a watched state change.
pub fn on_sync(response: &matrix_sdk::sync::SyncResponse) {
    let shown: Vec<(QtThread, String)> = match views().lock() {
        Ok(v) => v.values().filter(|(_, s)| !s.is_empty()).map(|(q, s)| (q.clone(), s.clone())).collect(),
        Err(_) => return,
    };
    if shown.is_empty() {
        return;
    }
    for (rid, upd) in &response.rooms.joined {
        let room = rid.as_str();
        if !shown.iter().any(|(_, s)| s == room) {
            continue;
        }
        let mut hit = false;
        let mut look = |v: &Value| {
            let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
            if v.get("state_key").is_some() && WATCH.contains(&ty) {
                hit = true;
            }
        };
        for ev in &upd.timeline.events {
            if let Ok(v) = ev.raw().deserialize_as::<Value>() {
                look(&v);
            }
        }
        let state = match &upd.state {
            matrix_sdk::sync::State::Before(v) | matrix_sdk::sync::State::After(v) => v,
        };
        for raw in state {
            if let Ok(v) = raw.deserialize_as::<Value>() {
                look(&v);
            }
        }
        if hit {
            for (qt, s) in &shown {
                if s == room {
                    spawn_read(qt.clone(), s.clone());
                }
            }
        }
    }
}

fn spawn_read(qt: QtThread, space: String) {
    crate::bridge::runtime().spawn(async move {
        let v = read(&space).await;
        let s = v.to_string();
        let _ = qt.queue(move |mut o| {
            if o.space_id().to_string() == space {
                o.as_mut().set_note_json(QString::from(&s));
            }
        });
    });
}

/// The note of `space` as the widget shows it.
async fn read(space: &str) -> Value {
    let Some(client) = crate::session::client() else { return json!({ "space": space, "text": null, "canEdit": false }) };
    let Ok(rid) = matrix_sdk::ruma::RoomId::parse(space) else { return json!({ "space": space, "text": null, "canEdit": false }) };
    let Some(room) = client.get_room(&rid) else { return json!({ "space": space, "text": null, "canEdit": false }) };
    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let space_name = match room.display_name().await {
        Ok(d) => d.to_string(),
        Err(_) => room.name().unwrap_or_default(),
    };
    let ev = match room.get_state_event(matrix_sdk::ruma::events::StateEventType::from(NOTE_TYPE), "").await {
        Ok(Some(raw)) => crate::appstate::raw_to_value(&raw),
        _ => None,
    };
    let can_edit = crate::roles::effective_perms(space, &me).await.has("edit_space_note");
    let mut out = json!({ "space": space, "spaceName": space_name, "text": null, "canEdit": can_edit });
    if let Some(ev) = ev {
        if let Some(text) = note_of(ev.get("content").unwrap_or(&Value::Null)) {
            let sender = ev.get("sender").and_then(Value::as_str).unwrap_or("").to_string();
            let ts = ev.get("origin_server_ts").and_then(Value::as_f64).unwrap_or(0.0);
            let name = crate::room::resolve_member_name(&room, &sender).await;
            let avatar = crate::room::sender_avatar(&room, &sender, &mut HashMap::new()).await;
            out["text"] = Value::String(text);
            out["sender"] = Value::String(sender);
            out["name"] = Value::String(name);
            out["avatar"] = Value::String(avatar);
            out["ts"] = tinhat_widgets::num(ts);
        }
    }
    out
}

impl qobject::NoteController {
    fn view_id(mut self: Pin<&mut Self>) -> u64 {
        if self.rust().id == 0 {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            self.as_mut().rust_mut().id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        self.rust().id
    }

    pub fn show(mut self: Pin<&mut Self>, space_id: &QString) {
        let space = space_id.to_string();
        let id = self.as_mut().view_id();
        let qt = self.qt_thread();
        if let Ok(mut v) = views().lock() {
            v.insert(id, (qt.clone(), space.clone()));
        }
        let changed = self.space_id().to_string() != space;
        self.as_mut().set_space_id(space_id.clone());
        if changed {
            self.as_mut().set_note_json(QString::default());
            self.as_mut().set_error(QString::default());
        }
        if space.is_empty() {
            self.as_mut().set_note_json(QString::from(&json!({ "space": "", "text": null, "canEdit": false }).to_string()));
            return;
        }
        spawn_read(qt, space);
    }

    pub fn close(mut self: Pin<&mut Self>) {
        let id = self.as_mut().view_id();
        if let Ok(mut v) = views().lock() {
            v.remove(&id);
        }
    }

    pub fn save(mut self: Pin<&mut Self>, text: &QString) {
        let space = self.space_id().to_string();
        if space.is_empty() || *self.saving() {
            return;
        }
        let content = match note_to_save(&text.to_string()) {
            Ok(c) => c,
            Err(_) => {
                self.as_mut().set_error(QString::from("That's over 500 characters — shorten it to save."));
                return;
            }
        };
        self.as_mut().set_saving(true);
        self.as_mut().set_error(QString::default());
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let res = async {
                let client = crate::session::client().ok_or("not signed in")?;
                let rid = matrix_sdk::ruma::RoomId::parse(&space).map_err(|_| "no such space")?;
                let room = client.get_room(&rid).ok_or("no such space")?;
                room.send_state_event_raw(NOTE_TYPE, "", content).await.map_err(|e| {
                    let s = e.to_string();
                    if s.contains("M_FORBIDDEN") || s.contains("403") {
                        "You can't edit this space's note"
                    } else {
                        "Couldn't save the note — check your connection and try again"
                    }
                })?;
                Ok::<(), &'static str>(())
            }
            .await;
            let v = read(&space).await.to_string();
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_saving(false);
                match res {
                    Ok(()) => o.as_mut().set_error(QString::default()),
                    Err(e) => o.as_mut().set_error(QString::from(e)),
                }
                if o.space_id().to_string() == space {
                    o.as_mut().set_note_json(QString::from(&v));
                }
            });
        });
    }

    pub fn counter(&self, text: &QString) -> QString {
        QString::from(&note_counter_json(&text.to_string()).to_string())
    }
    pub fn segments(&self, text: &QString) -> QString {
        QString::from(&note_segments_json(&text.to_string()).to_string())
    }
    pub fn first_line(&self, text: &QString) -> QString {
        QString::from(&note_first_line(Some(&text.to_string())))
    }
}
