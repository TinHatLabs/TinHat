//! GameNightController — the Game night widget's helpers (widgets W2, contract WIDGETS.md §23; the web's
//! features/widgets/gamenight/GameNight.tsx). The widget reads the calendar through Up next's own WidgetsController
//! (`upNext`) and the Game servers link through GameServersController; this adds the shared game-night rules
//! (`tinhat_widgets::gamenight`, the web's gamenight.ts: which events are game nights, the countdown, the Small line,
//! the credit line) and MY Game Reel credit for the hero's space (`loadCredit` → `creditJson`; games.rs `my_credit`:
//! the Reel's own computation, read-only — no power-level write). QML asks for the credit when the hero changes and
//! every 10 minutes while the window is not hidden.

use core::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;
use serde_json::{json, Value};
use tinhat_widgets::gamenight as gn;

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
        #[qproperty(QString, credit_json)] // QML: creditJson — {key: "container/session", credit (null = unknown), mode} ("" = none)
        type GameNightController = super::GameNightControllerRust;

        /// QML: loadCredit(container, sessionId) — read my Reel credit for that night (async → creditJson).
        #[qinvokable]
        fn load_credit(self: Pin<&mut GameNightController>, container: &QString, session_id: &QString);
        /// QML: selectNights(itemsJson, nowMs, max) — the game nights, soonest first (gamenight::select_nights).
        #[qinvokable]
        fn select_nights(self: &GameNightController, items_json: &QString, now_ms: f64, max: i32) -> QString;
        /// QML: nightWhen(start, end, nowMs, dayTime) — "now", "in 2 h 10 m" or the app's "Sat 7:30 PM".
        #[qinvokable]
        fn night_when(self: &GameNightController, start: f64, end: f64, now_ms: f64, day_time: &QString) -> QString;
        /// QML: nightSummary(has, game, when) — the Small line after the header's title ("Valheim · in 2 h").
        #[qinvokable]
        fn night_summary(self: &GameNightController, has: bool, game: &QString, when: &QString) -> QString;
        /// QML: creditText(creditJson) — "Your credit: +2", "Your credit: 0", "Credits are off tonight", "".
        #[qinvokable]
        fn credit_text(self: &GameNightController, credit_json: &QString) -> QString;
        /// QML: coverArt(gameId) — the games catalogue's portrait art URL (Up next's covers; the samples).
        #[qinvokable]
        fn cover_art(self: &GameNightController, game_id: &QString) -> QString;
    }

    impl cxx_qt::Threading for GameNightController {}
}

#[derive(Default)]
pub struct GameNightControllerRust {
    credit_json: QString,
}

impl qobject::GameNightController {
    pub fn load_credit(self: Pin<&mut Self>, container: &QString, session_id: &QString) {
        let (cid, sid) = (container.to_string(), session_id.to_string());
        if cid.is_empty() || sid.is_empty() || cid.ends_with(":sample.invalid") || crate::session::client().is_none() {
            return;
        }
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let key = format!("{cid}/{sid}");
            let v = match crate::games::my_credit(&cid, &sid).await {
                Some((credit, mode)) => json!({ "key": key, "credit": tinhat_widgets::num(credit), "mode": mode }),
                None => json!({ "key": key, "credit": Value::Null, "mode": "" }),
            };
            let text = v.to_string();
            let _ = qt.queue(move |mut o| o.as_mut().set_credit_json(QString::from(&text)));
        });
    }

    pub fn select_nights(&self, items_json: &QString, now_ms: f64, max: i32) -> QString {
        let v = tinhat_theme::json::parse_json(&items_json.to_string()).unwrap_or(Value::Null);
        let out: Vec<Value> = gn::select_nights(&v, now_ms, max.max(0) as usize).iter().map(gn::NightItem::to_json).collect();
        QString::from(&Value::Array(out).to_string())
    }

    pub fn night_when(&self, start: f64, end: f64, now_ms: f64, day_time: &QString) -> QString {
        QString::from(&gn::night_when(start, end, now_ms, &day_time.to_string()))
    }

    pub fn night_summary(&self, has: bool, game: &QString, when: &QString) -> QString {
        QString::from(&gn::night_summary(has, &game.to_string(), &when.to_string()))
    }

    pub fn cover_art(&self, game_id: &QString) -> QString {
        QString::from(&crate::games::art_url(&crate::games::default_catalog_base(), &game_id.to_string(), "portrait"))
    }

    pub fn credit_text(&self, credit_json: &QString) -> QString {
        let v = tinhat_theme::json::parse_json(&credit_json.to_string()).unwrap_or(Value::Null);
        if !v.is_object() {
            return QString::default();
        }
        QString::from(&gn::credit_text(v.get("credit").and_then(Value::as_f64), v.get("mode").and_then(Value::as_str).unwrap_or("")))
    }
}
