//! FFI to the Windows QVideoSink prototype shim (`qvideo_bridge.cpp`) + the
//! `TINHAT_QVS_TEST` self-test driver. Windows-only (the .cpp is only compiled there);
//! the module itself is `#[cfg(windows)]`-gated in main.rs.
//!
//! Prototype 2 of HANDOFF §6: prove `appsink → QVideoFrame → QVideoSink → QML
//! VideoOutput` renders under the default D3D11 RHI, with a headless-friendly
//! verdict (frames pushed vs frames the sink consumed) in the TINHAT_CAM_GLTEST
//! self-test tradition.

use std::os::raw::{c_int, c_long, c_void};

extern "C" {
    fn tinhat_qvs_open_window() -> *mut c_void;
    fn tinhat_qvs_push_i420(
        sink: *mut c_void,
        y: *const u8,
        y_stride: c_int,
        u: *const u8,
        u_stride: c_int,
        v: *const u8,
        v_stride: c_int,
        width: c_int,
        height: c_int,
    ) -> c_int;
    fn tinhat_qvs_frames_seen() -> c_long;
}

/// Run the QVideoSink prototype: open the test window, feed `videotestsrc` I420
/// frames from an appsink for ~8s on a worker thread, then print a verdict and
/// exit the process. Call INSTEAD of loading Main.qml (after QGuiApplication +
/// gst init); the Qt event loop must be started by the caller afterwards.
pub fn run_selftest_pipeline() {
    use gstreamer as gst;
    use gstreamer::prelude::*;

    let sink_ptr = unsafe { tinhat_qvs_open_window() };
    if sink_ptr.is_null() {
        eprintln!("[qvs] VERDICT: FAIL — no QVideoSink (window/QML setup failed)");
        crate::platform::hard_exit(1);
    }
    // Send the raw pointer to the worker thread (QVideoSink lives for the process).
    let sink_addr = sink_ptr as usize;

    std::thread::spawn(move || {
        let pipeline = match gst::parse::launch(
            "videotestsrc is-live=true pattern=ball ! videoconvert ! \
             video/x-raw,format=I420,width=640,height=360,framerate=30/1 ! \
             appsink name=sink emit-signals=false max-buffers=2 drop=true sync=true",
        ) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[qvs] VERDICT: FAIL — pipeline parse: {e}");
                crate::platform::hard_exit(1);
            }
        };
        let bin = pipeline.downcast_ref::<gst::Bin>().unwrap();
        let appsink = bin
            .by_name("sink")
            .unwrap()
            .downcast::<gstreamer_app::AppSink>()
            .unwrap();
        pipeline.set_state(gst::State::Playing).ok();

        let mut pushed: u64 = 0;
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(8) {
            let Ok(sample) = appsink.pull_sample() else { break };
            let Some(buffer) = sample.buffer() else { continue };
            let Some(caps) = sample.caps() else { continue };
            let Ok(vinfo) = gstreamer_video::VideoInfo::from_caps(caps) else { continue };
            let Ok(vframe) = gstreamer_video::VideoFrameRef::from_buffer_ref_readable(buffer, &vinfo)
            else {
                continue;
            };
            let (w, h) = (vinfo.width() as i32, vinfo.height() as i32);
            let ok = unsafe {
                tinhat_qvs_push_i420(
                    sink_addr as *mut c_void,
                    vframe.plane_data(0).unwrap().as_ptr(),
                    vinfo.stride()[0],
                    vframe.plane_data(1).unwrap().as_ptr(),
                    vinfo.stride()[1],
                    vframe.plane_data(2).unwrap().as_ptr(),
                    vinfo.stride()[2],
                    w,
                    h,
                )
            };
            if ok == 1 {
                pushed += 1;
            }
        }
        pipeline.set_state(gst::State::Null).ok();
        // Give queued setVideoFrame calls a beat to drain on the GUI thread.
        std::thread::sleep(std::time::Duration::from_millis(500));
        let seen = unsafe { tinhat_qvs_frames_seen() };
        let pass = seen >= 90; // ~3s worth at 30fps actually consumed by the sink
        eprintln!(
            "[qvs] VERDICT: {} — pushed {pushed} frames, QVideoSink consumed {seen} (PASS needs >=90)",
            if pass { "PASS" } else { "FAIL" }
        );
        // hard_exit, not process::exit: the GUI + D3D11 render threads are live; see
        // platform::hard_exit (the Windows twin of the Linux gates' libc::_exit).
        crate::platform::hard_exit(if pass { 0 } else { 1 });
    });
}
