//! CamGlTest — env-gated (`TINHAT_CAM_GLTEST`) self-test that drives the REAL camera selfview
//! pipeline (`build_camera_selfview_pipeline`) against a REAL `qml6glsink` in a throwaway QML
//! window (`CamGlTest.qml`), WITHOUT a call or any UI interaction. This is the only way to
//! reproduce the camera-on `gst_gl_upload` SIGSEGV: the crash is specific to Qt's shared GLES2
//! context (qml6glsink) + a live signal — a standalone `glimagesink` gst-launch can't reproduce
//! it. main.rs loads CamGlTest.qml instead of Main.qml when `TINHAT_CAM_GLTEST` is set; the QML
//! calls start() (build + attach the selfview to a `vid_selfview` container) then, after a few
//! seconds, finishOk() (clean teardown + exit 0 = no crash). A glupload SIGSEGV instead kills
//! the process mid-run (before "SURVIVED" is logged).
//!
//! Pair with `TINHAT_CAM_FORCE=WxH` (read in streamer `camera::camera_src_chain`) to choose the
//! capture mode: 1280x720 reproduces the passthrough crash; 1920x1080 validates the downscale fix.
use core::pin::Pin;
use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qobject {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name] // snake_case Rust -> camelCase QML (start, finishOk)
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, device)] // QML: device — v4l2 node to test, or "" for default
        type CamGlTest = super::CamGlTestRust;

        /// QML: camGlTest.start() — build the selfview pipeline + attach it to the
        /// `vid_selfview` container (the real qml6glsink GL-upload path), then run.
        #[qinvokable]
        fn start(self: Pin<&mut CamGlTest>);
        /// QML: camGlTest.finishOk() — reached only if no in-run glupload crash; tears the
        /// pipeline down cleanly then exit(0) = PASS.
        #[qinvokable]
        fn finish_ok(self: &CamGlTest);
    }
}

pub struct CamGlTestRust {
    device: QString,
    ptr: usize, // the selfview pipeline's GstElement* (for predetach on clean teardown)
}

impl Default for CamGlTestRust {
    fn default() -> Self {
        // TINHAT_CAM_GLTEST is either "1"/"default"/empty (default device) or a /dev/videoN path.
        let dev = std::env::var("TINHAT_CAM_GLTEST").unwrap_or_default();
        CamGlTestRust {
            device: QString::from(&dev),
            ptr: 0,
        }
    }
}

impl qobject::CamGlTest {
    pub fn start(mut self: Pin<&mut Self>) {
        let dev = self.device().to_string();
        let dev_opt = if dev.is_empty() || dev == "1" || dev == "default" {
            None
        } else {
            Some(dev.as_str())
        };
        eprintln!(
            "[camgltest] building selfview pipeline (device={dev_opt:?}, TINHAT_CAM_FORCE={:?})",
            std::env::var("TINHAT_CAM_FORCE").ok()
        );
        match tinhat_streamer::camera::cam_gltest_selfview_ptr(dev_opt) {
            Ok(ptr) => {
                self.as_mut().rust_mut().ptr = ptr;
                let ok = crate::video::attach(ptr, "vid_selfview");
                eprintln!(
                    "[camgltest] attach(vid_selfview) -> {ok}; running, watching for glupload SIGSEGV..."
                );
            }
            Err(e) => {
                eprintln!("[camgltest] pipeline build FAILED: {e}");
                std::process::exit(2);
            }
        }
    }

    pub fn finish_ok(&self) {
        eprintln!("[camgltest] SURVIVED — selfview glupload ran with no crash. PASS.");
        // Clean teardown so a PASS exits 0 (not a leaked-PLAYING-pipeline teardown segfault):
        // unbind the qml6glsink widget on this (GUI) thread, THEN set the pipeline to Null.
        let ptr = self.rust().ptr;
        if ptr != 0 {
            crate::video::predetach(ptr);
        }
        tinhat_streamer::camera::cam_gltest_stop();
        eprintln!("[camgltest] pipeline torn down cleanly; exiting 0.");
        std::process::exit(0);
    }
}
