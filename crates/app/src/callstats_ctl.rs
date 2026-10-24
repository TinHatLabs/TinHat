//! CallStatsController — the Call stats widget's live data (widgets, modular UI step 2; contract WIDGETS.md §5–§6).
//!
//! The numbers are made in the streamer (`tinhat_streamer::callstats`: receive stats per remote track, LiveKit
//! ConnectionQuality, the `tinhat.rxq` reports both ways). This controller only hands its snapshot to QML and tells
//! the streamer how the window is doing:
//!
//! * `setWindow("focused" | "unfocused" | "hidden")` — the sampler runs every 1 s focused, every 5 s otherwise; while
//!   hidden no snapshot reaches QML (nothing to draw), and showing the window again hands over the latest at once.
//! * `json` changes once per sample (never on a timer of its own): the streamer calls the hook installed by
//!   `start()`, which queues one property write onto the GUI thread.
//!
//! One instance per process (the widget host creates it once); a second `start()` re-points the hook at the newest.

use core::pin::Pin;
use std::sync::Arc;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;

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
        #[qproperty(QString, json)] //         QML: json — the call-stats snapshot ("" outside a call; see streamer callstats.rs build_snapshot)
        #[qproperty(QString, window_state)] // QML: windowState — the state last passed to setWindow
        #[qproperty(i32, rev)] //              QML: rev — bumps with every snapshot handed over
        type CallStatsController = super::CallStatsControllerRust;

        /// QML: start() — install the snapshot hook (idempotent).
        #[qinvokable]
        fn start(self: Pin<&mut CallStatsController>);
        /// QML: setWindow(state) — "focused" | "unfocused" | "hidden" (the app-wide window state, GPU.md §1).
        #[qinvokable]
        fn set_window(self: Pin<&mut CallStatsController>, state: &QString);
        /// QML: refresh() — read the latest snapshot now (the hook does this after every sample).
        #[qinvokable]
        fn refresh(self: Pin<&mut CallStatsController>);
        /// QML: setActive(on) — a Call stats widget (or the gallery's preview) shows the snapshot: while off, samples
        /// are not handed to QML at all (the streamer keeps sampling and reporting — the others' widgets need it).
        #[qinvokable]
        fn set_active(self: Pin<&mut CallStatsController>, on: bool);
    }

    impl cxx_qt::Threading for CallStatsController {}
}

pub struct CallStatsControllerRust {
    json: QString,
    window_state: QString,
    rev: i32,
    started: bool,
    active: bool,
}

impl Default for CallStatsControllerRust {
    fn default() -> Self {
        Self { json: QString::default(), window_state: QString::from("focused"), rev: 0, started: false, active: false }
    }
}

impl qobject::CallStatsController {
    pub fn start(mut self: Pin<&mut Self>) {
        if self.rust().started {
            return;
        }
        self.as_mut().rust_mut().started = true;
        let qt = self.qt_thread();
        tinhat_streamer::callstats::set_snapshot_hook(Arc::new(move || {
            let _ = qt.queue(|o| o.refresh());
        }));
        self.refresh();
    }

    pub fn set_window(mut self: Pin<&mut Self>, state: &QString) {
        let s = state.to_string();
        let s = match s.as_str() {
            "hidden" | "unfocused" | "focused" => s,
            _ => "focused".to_string(),
        };
        if self.window_state().to_string() != s {
            self.as_mut().set_window_state(QString::from(&s));
        }
        // after the property: leaving "hidden" makes the streamer hand over its latest snapshot at once
        tinhat_streamer::callstats::set_view(&s);
    }

    pub fn set_active(mut self: Pin<&mut Self>, on: bool) {
        self.as_mut().rust_mut().active = on;
        if on {
            self.refresh();
        }
    }

    pub fn refresh(mut self: Pin<&mut Self>) {
        if !self.rust().active || self.window_state().to_string() == "hidden" {
            return; // nobody shows it / paused: nothing is drawn while hidden (resumes through the hook on show)
        }
        let j = tinhat_streamer::callstats::snapshot_json();
        if self.json().to_string() != j {
            self.as_mut().set_json(QString::from(&j));
            let r = self.rev().wrapping_add(1);
            self.as_mut().set_rev(r);
        }
    }
}
