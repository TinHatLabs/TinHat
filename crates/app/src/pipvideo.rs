//! PipVideoItem — the pop-out (PiP) video surface: a QQuickPaintedItem that paints RGBA
//! frames delivered by the streamer's PiP viewer tap (`pip_tap_start`). NO GstGL and NO
//! qml6glsink anywhere in this path.
//!
//! WHY a CPU-painted item: qml6glsink in a second QQuickWindow depends on cross-context
//! GL texture sharing, which is driver-dependent — Qt 6.11 silently drops EGL sharing
//! when share-creation fails, and gstqsg6material adopts foreign texture ids unchecked,
//! sampling opaque black with zero errors (the owner's NVIDIA desktop bug that r137's
//! rebuild architecture could not fix; Intel/Mesa masked it). Here Qt owns every GL
//! object in the PiP window's own context; the only cross-thread hand-off is a plain
//! RGBA buffer. Cost: one libyuv convert + one texture upload per frame, only while a
//! pop-out is open — trivial next to the stream's decode.
//!
//! Delivery is latest-wins: the router parks the newest frame in a per-item slot and
//! queues at most ONE GUI-thread hop at a time, so a stalled GUI thread coalesces
//! frames instead of queueing 8MB closures.

use core::pin::Pin;
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{QImage, QImageFormat, QRect, QString};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qpainter.h");
        type QPainter = cxx_qt_lib::QPainter;
    }

    // The QtQuick base type (same shape as cxx-qt's custom_parent_class example).
    unsafe extern "C++" {
        include!(<QtQuick/QQuickPaintedItem>);
        /// Base class: Qt rasterizes our QPainter output and textures it itself.
        type QQuickPaintedItem;
    }

    #[auto_cxx_name] // snake_case Rust -> camelCase QML (attachViewer, detachViewer)
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[base = QQuickPaintedItem]
        #[qproperty(QString, tile_id)] // QML: tileId — the grid tile id whose track we view
        #[qproperty(bool, has_frame)]  // QML: hasFrame — 10.1 (TH-POP-17): a frame has arrived (else "Waiting for video…")
        type PipVideoItem = super::PipVideoItemRust;

        /// QQuickPaintedItem::paint — draw the latest frame letterboxed into the item.
        #[qinvokable]
        #[cxx_override]
        unsafe fn paint(self: Pin<&mut PipVideoItem>, painter: *mut QPainter);

        /// QQuickItem::update() — schedule a repaint (GUI thread).
        #[inherit]
        fn update(self: Pin<&mut PipVideoItem>);
        /// QQuickItem::width()/height() — current item geometry.
        #[inherit]
        fn width(self: &PipVideoItem) -> f64;
        #[inherit]
        fn height(self: &PipVideoItem) -> f64;

        /// QML (Component.onCompleted): register under `tileId` so routed frames reach us.
        /// ("register" itself is a C++ keyword, hence the longer names.)
        #[qinvokable]
        fn attach_viewer(self: Pin<&mut PipVideoItem>);
        /// QML (Component.onDestruction): drop out of the frame registry.
        #[qinvokable]
        fn detach_viewer(self: Pin<&mut PipVideoItem>);
    }

    // Default-construct via Initialize (QML creates elements with the default ctor;
    // the plain generated ctor would pass a QObject* parent to the QQuickItem base).
    impl cxx_qt::Initialize for PipVideoItem {}

    impl cxx_qt::Threading for PipVideoItem {}
}

pub struct PipVideoItemRust {
    tile_id: QString,
    has_frame: bool,
    /// Latest delivered frame, ready to paint (owns its RGBA bytes via from_raw_bytes).
    frame: Option<QImage>,
    frame_size: (i32, i32),
    painted_once: bool,
    /// The registry key we actually registered under (survives tile_id churn).
    registered_as: Option<String>,
}

impl Default for PipVideoItemRust {
    fn default() -> Self {
        PipVideoItemRust {
            has_frame: false,
            tile_id: QString::default(),
            frame: None,
            frame_size: (0, 0),
            painted_once: false,
            registered_as: None,
        }
    }
}

/// One registered viewer: how to reach its GUI thread + the latest-wins frame slot.
struct ViewerEntry {
    thread: cxx_qt::CxxQtThread<qobject::PipVideoItem>,
    pending: Arc<Mutex<Option<(u32, u32, Vec<u8>)>>>,
    queued: Arc<AtomicBool>,
}

fn viewers() -> &'static Mutex<HashMap<String, ViewerEntry>> {
    static V: OnceLock<Mutex<HashMap<String, ViewerEntry>>> = OnceLock::new();
    V.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Install the streamer->app frame router (call once at startup, before any call).
/// Runs on the tap's tokio thread: park the frame, then queue ONE GUI hop.
pub fn install_pip_router() {
    tinhat_streamer::call::set_pip_frame_router(Box::new(|id, w, h, rgba| {
        let reg = viewers().lock().unwrap();
        let Some(entry) = reg.get(id) else { return };
        *entry.pending.lock().unwrap() = Some((w, h, rgba)); // latest-wins
        if !entry.queued.swap(true, Ordering::AcqRel) {
            let pending = entry.pending.clone();
            let queued = entry.queued.clone();
            if entry
                .thread
                .queue(move |mut item| {
                    queued.store(false, Ordering::Release);
                    let taken = pending.lock().unwrap().take();
                    if let Some((w, h, data)) = taken {
                        item.as_mut().deliver(w, h, data);
                    }
                })
                .is_err()
            {
                // Item's thread is gone (teardown race) — allow future attempts.
                entry.queued.store(false, Ordering::Release);
            }
        }
    }));
}

impl cxx_qt::Initialize for qobject::PipVideoItem {
    fn initialize(self: Pin<&mut Self>) {}
}

impl qobject::PipVideoItem {
    /// GUI thread (queued from the router): adopt the frame and schedule a repaint.
    fn deliver(mut self: Pin<&mut Self>, w: u32, h: u32, data: Vec<u8>) {
        // SAFETY: data is exactly w*h*4 RGBA bytes (the tap sized it); from_raw_bytes
        // hands the Vec's ownership to the QImage (freed by its cleanup fn).
        let img = unsafe {
            QImage::from_raw_bytes(data, w as i32, h as i32, QImageFormat::Format_RGBA8888)
        };
        let first = self.rust().frame.is_none();
        self.as_mut().rust_mut().frame = Some(img);
        self.as_mut().rust_mut().frame_size = (w as i32, h as i32);
        if first {
            eprintln!("[pip] first frame delivered to viewer ({w}x{h})");
            self.as_mut().set_has_frame(true);
        }
        self.as_mut().update();
    }

    pub fn attach_viewer(mut self: Pin<&mut Self>) {
        let id = self.tile_id().to_string();
        if id.is_empty() {
            eprintln!("[pip] attach_viewer with empty tileId — ignored");
            return;
        }
        let entry = ViewerEntry {
            thread: self.qt_thread(),
            pending: Arc::new(Mutex::new(None)),
            queued: Arc::new(AtomicBool::new(false)),
        };
        viewers().lock().unwrap().insert(id.clone(), entry);
        eprintln!("[pip] viewer registered for {id}");
        self.as_mut().rust_mut().registered_as = Some(id);
    }

    pub fn detach_viewer(mut self: Pin<&mut Self>) {
        if let Some(id) = self.as_mut().rust_mut().registered_as.take() {
            viewers().lock().unwrap().remove(&id);
            eprintln!("[pip] viewer unregistered for {id}");
        }
    }

    /// # Safety
    /// Derefs the raw QPainter Qt hands the paint override (never null in practice).
    pub unsafe fn paint(mut self: Pin<&mut Self>, painter: *mut cxx_qt_lib::QPainter) {
        let Some(painter) = (unsafe { painter.as_mut() }) else { return };
        let mut painter = unsafe { Pin::new_unchecked(painter) };
        let (iw, ih) = (self.width(), self.height());
        let (fw, fh) = self.rust().frame_size;
        if iw <= 0.0 || ih <= 0.0 || fw <= 0 || fh <= 0 {
            return; // nothing to show yet — the window's black background stands in
        }
        // Letterbox: fit the frame inside the item, centered, aspect preserved.
        let scale = f64::min(iw / fw as f64, ih / fh as f64);
        let dw = ((fw as f64 * scale).round() as i32).max(1);
        let dh = ((fh as f64 * scale).round() as i32).max(1);
        let dx = ((iw as i32) - dw) / 2;
        let dy = ((ih as i32) - dh) / 2;
        let rect = QRect::new(dx, dy, dw, dh);
        painter
            .as_mut()
            .set_render_hint(cxx_qt_lib::QPainterRenderHint::SmoothPixmapTransform, true);
        {
            let Some(img) = self.rust().frame.as_ref() else { return };
            painter.as_mut().draw_image(&rect, img);
        }
        if !self.rust().painted_once {
            self.as_mut().rust_mut().painted_once = true;
            // The diagnostics line that PROVES pixels were drawn (not just delivered) —
            // the r137 desktop log could only prove buffer delivery to the GL sink.
            eprintln!(
                "[pip] first frame PAINTED {fw}x{fh} -> item {:.0}x{:.0} (CPU path)",
                iw, ih
            );
        }
    }
}
