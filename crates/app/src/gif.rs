//! In-chat GIF picker (Giphy via the server's Tin Hat API) — Phase 3, a port of the fork's
//! `GifButton.tsx`. Trending on open, debounced search as you type. Picking a GIF sends
//! its Giphy CDN URL as a PLAIN `m.text` whose inline embed (ThEmbedCard, Phase 2)
//! renders the GIF in place — an ordinary, deletable message with no homeserver upload.
//!
//! The proxy returns `[{id,title,url,width,height,preview}]`; QML shows the `preview`
//! thumbnails in a grid and sends `url` on pick.

use core::pin::Pin;
use std::sync::Mutex;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use tokio::task::JoinHandle;

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
        #[qproperty(QString, results_json)] // QML: resultsJson — [{id,title,url,width,height,preview}]
        #[qproperty(bool, loading)] //          QML: loading
        #[qproperty(QString, error)] //         QML: error — "" or a user-facing message
        type GifController = super::GifControllerRust;

        /// QML: search(query) — empty/whitespace = trending (immediate); non-empty is
        /// debounced 450ms (and aborts the prior in-flight fetch), mirroring the fork.
        #[qinvokable]
        fn search(self: Pin<&mut GifController>, query: &QString);
        /// QML: pick(roomId, url) — send the GIF's CDN URL as a plain m.text message.
        #[qinvokable]
        fn pick(self: Pin<&mut GifController>, room_id: &QString, url: &QString);
        /// QML: clear() — reset results/error/loading (on close).
        #[qinvokable]
        fn clear(self: Pin<&mut GifController>);
    }

    impl cxx_qt::Threading for GifController {}
}

#[derive(Default)]
pub struct GifControllerRust {
    results_json: QString,
    loading: bool,
    error: QString,
    // The most-recent in-flight search task, so a new keystroke aborts the previous one.
    inflight: Mutex<Option<JoinHandle<()>>>,
}

impl qobject::GifController {
    pub fn search(mut self: Pin<&mut Self>, query: &QString) {
        let q = query.to_string().trim().to_string();
        self.as_mut().set_loading(true);
        self.as_mut().set_error(QString::default());
        let qt = self.qt_thread();
        let handle = crate::bridge::runtime().spawn(async move {
            // Debounce typing (trending opens immediately).
            if !q.is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(450)).await;
            }
            let path = if q.is_empty() {
                "/gifs/trending".to_string()
            } else {
                format!("/gifs/search?q={}", enc(&q))
            };
            let result = crate::proxy::get_json(&path).await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_loading(false);
                match result {
                    Ok(v) => {
                        let json = serde_json::to_string(&v).unwrap_or_else(|_| "[]".to_string());
                        o.as_mut().set_results_json(QString::from(&json));
                    }
                    Err(e) => {
                        eprintln!("[gif] {e}");
                        o.as_mut().set_error(QString::from("Couldn't load GIFs"));
                        o.as_mut().set_results_json(QString::from("[]"));
                    }
                }
            });
        });
        // Abort the previous in-flight fetch (debounce), keep the new handle.
        if let Ok(mut g) = self.rust().inflight.lock() {
            if let Some(prev) = g.replace(handle) {
                prev.abort();
            }
        }
    }

    pub fn pick(self: Pin<&mut Self>, room_id: &QString, url: &QString) {
        let room_id = room_id.to_string();
        let url = url.to_string();
        crate::bridge::runtime().spawn(async move {
            if let Err(e) = crate::room::do_send_plain(&room_id, &url).await {
                eprintln!("[gif] send failed: {e}");
            }
        });
    }

    pub fn clear(mut self: Pin<&mut Self>) {
        self.as_mut().set_results_json(QString::default());
        self.as_mut().set_error(QString::default());
        self.as_mut().set_loading(false);
    }
}

/// Minimal RFC-3986 percent-encode for a query value.
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Headless round-trip (no GUI): fetch trending (or `TINHAT_GIF_QUERY`) via the proxy and
/// print the count + first item, proving the proxy contract + JSON shape. If
/// `TINHAT_GIF_SEND='!room:server'` is set, restore the session and send the first GIF.
///   TINHAT_GIF_TEST=1 [TINHAT_GIF_QUERY=cats] [TINHAT_GIF_SEND='!room:server'] ./tinhat
pub async fn headless_verify() {
    let q = std::env::var("TINHAT_GIF_QUERY").unwrap_or_default();
    let path = if q.trim().is_empty() {
        "/gifs/trending".to_string()
    } else {
        format!("/gifs/search?q={}", enc(q.trim()))
    };
    eprintln!("[giftest] GET {}{path}", crate::proxy::base());
    let v = match crate::proxy::get_json(&path).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[giftest] FAIL: {e}");
            return;
        }
    };
    let arr = v.as_array().cloned().unwrap_or_default();
    eprintln!("[giftest] {} result(s)", arr.len());
    if let Some(first) = arr.first() {
        eprintln!(
            "[giftest] first: id={} url={} preview={}",
            first.get("id").and_then(|x| x.as_str()).unwrap_or("?"),
            first.get("url").and_then(|x| x.as_str()).unwrap_or("?"),
            first.get("preview").and_then(|x| x.as_str()).unwrap_or("?"),
        );
    }
    let ok = !arr.is_empty()
        && arr.first().map(|f| f.get("url").and_then(|u| u.as_str()).is_some()).unwrap_or(false);
    eprintln!("[giftest] VERDICT: {}", if ok { "PASS (proxy + shape)" } else { "FAIL" });

    if let Ok(room) = std::env::var("TINHAT_GIF_SEND") {
        let url = arr.first().and_then(|f| f.get("url")).and_then(|u| u.as_str()).unwrap_or("");
        if url.is_empty() {
            eprintln!("[giftest] no url to send");
            return;
        }
        match crate::bridge::restore_client().await {
            Ok(c) => {
                crate::session::set_client(c.clone());
                let _ = c
                    .sync_once(crate::bridge::sync_settings().timeout(std::time::Duration::from_secs(10)))
                    .await;
                match crate::room::do_send_plain(&room, url).await {
                    Ok(()) => eprintln!("[giftest] SENT to {room}: {url}"),
                    Err(e) => eprintln!("[giftest] send FAILED: {e}"),
                }
            }
            Err(e) => eprintln!("[giftest] restore failed: {e}"),
        }
    }
}
